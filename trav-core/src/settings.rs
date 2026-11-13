use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// User-tunable engine settings. Persisted as `settings.json` in the state dir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub download_dir: PathBuf,
    /// Listen port: TCP, and over UDP µTP and the DHT.
    pub listen_port: u16,
    pub max_peers_per_torrent: usize,
    pub max_peers_global: usize,
    /// Downloads beyond this many are queued (0 = unlimited).
    pub max_active_downloads: usize,
    /// Bytes per second, 0 = unlimited.
    pub download_limit: u64,
    pub upload_limit: u64,
    pub upload_slots: usize,
    pub enable_dht: bool,
    pub enable_pex: bool,
    /// Find peers on the local network (BEP 14).
    pub enable_lsd: bool,
    pub enable_upnp: bool,
    /// µTP connections (BEP 29) and NAT hole punching (BEP 55) alongside TCP.
    pub enable_utp: bool,
    /// Stop seeding once uploaded/size reaches this (0 = never stop).
    pub seed_ratio_limit: f64,
    /// With no upload limit set, cap uploads automatically whenever they start
    /// queueing on the uplink and slowing everything else (see `governor`).
    pub auto_upload_limit: bool,
    /// Peer connection encryption (MSE/PE).
    pub encryption: Encryption,
    /// Also announce to well-known public trackers (never for private torrents).
    pub public_trackers: bool,
    /// Alternative limits, bytes per second (0 = unlimited), in force while
    /// `alt_speed` is on or `alt_schedule` covers the local time.
    pub alt_download_limit: u64,
    pub alt_upload_limit: u64,
    pub alt_speed: bool,
    pub alt_schedule: bool,
    /// Schedule window in minutes after local midnight; `from > to` spans midnight.
    pub alt_from: u16,
    pub alt_to: u16,
    /// Days the schedule starts on: bit 0 = Sunday … bit 6 = Saturday.
    pub alt_days: u8,
    /// `.torrent` files saved here are added by themselves (see `watch`).
    pub watch_dir: Option<PathBuf>,
    /// Which defaults this file was written with (for migrations). Files from
    /// before versioning have none, so it defaults to 0 rather than current.
    #[serde(default)]
    pub version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            listen_port: 51413,
            max_peers_per_torrent: 150,
            max_peers_global: 600,
            max_active_downloads: 5,
            download_limit: 0,
            upload_limit: 0,
            upload_slots: 8,
            enable_dht: true,
            enable_pex: true,
            enable_lsd: true,
            enable_upnp: true,
            enable_utp: true,
            seed_ratio_limit: 0.0,
            auto_upload_limit: true,
            encryption: Encryption::Prefer,
            public_trackers: true,
            alt_download_limit: 512 * 1024,
            alt_upload_limit: 128 * 1024,
            alt_speed: false,
            alt_schedule: false,
            alt_from: 9 * 60,
            alt_to: 17 * 60,
            alt_days: 0b011_1110,
            watch_dir: None,
            version: Self::VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Encryption {
    /// Encrypt when the peer can, otherwise connect in the clear.
    #[default]
    Prefer,
    /// Only encrypted connections.
    Require,
    /// Never encrypt.
    Off,
}

fn default_download_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("downloads"))
}

impl Settings {
    pub const VERSION: u32 = 1;

    /// Bring settings saved by an older version up to date: values the user
    /// never changed move to the new defaults.
    pub fn migrated(mut self) -> Self {
        if self.version < 1 {
            // 0.3: more connections per torrent (80 → 150, 500 → 600 overall).
            if self.max_peers_per_torrent == 80 {
                self.max_peers_per_torrent = 150;
            }
            if self.max_peers_global == 500 {
                self.max_peers_global = 600;
            }
        }
        self.version = Self::VERSION;
        self
    }

    /// The limits in force at minute `minute` after local midnight of weekday
    /// `day` (0 = Sunday): (download, upload, whether the alternative ones).
    pub fn limits_at(&self, day: u8, minute: u16) -> (u64, u64, bool) {
        if self.alt_speed || (self.alt_schedule && self.schedule_covers(day % 7, minute % 1440)) {
            (self.alt_download_limit, self.alt_upload_limit, true)
        } else {
            (self.download_limit, self.upload_limit, false)
        }
    }

    fn schedule_covers(&self, day: u8, minute: u16) -> bool {
        let on = |d: u8| self.alt_days & (1 << d) != 0;
        let (from, to) = (self.alt_from % 1440, self.alt_to % 1440);
        if from == to {
            on(day)
        } else if from < to {
            on(day) && (from..to).contains(&minute)
        } else {
            // Spans midnight: the evening belongs to today, the early hours to
            // the day the window started on, yesterday.
            (minute >= from && on(day)) || (minute < to && on((day + 6) % 7))
        }
    }

    /// Clamp values that would wedge the engine.
    pub fn sanitized(mut self) -> Self {
        self.alt_from %= 1440;
        self.alt_to %= 1440;
        self.alt_days &= 0x7f;
        self.max_peers_per_torrent = self.max_peers_per_torrent.clamp(1, 2000);
        self.max_peers_global = self
            .max_peers_global
            .clamp(self.max_peers_per_torrent.min(50), 20_000);
        self.upload_slots = self.upload_slots.clamp(1, 200);
        if !self.seed_ratio_limit.is_finite() || self.seed_ratio_limit < 0.0 {
            self.seed_ratio_limit = 0.0;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduled(from: u16, to: u16, days: u8) -> Settings {
        Settings {
            download_limit: 1000,
            upload_limit: 100,
            alt_download_limit: 50,
            alt_upload_limit: 5,
            alt_schedule: true,
            alt_from: from,
            alt_to: to,
            alt_days: days,
            ..Settings::default()
        }
    }

    #[test]
    fn the_switch_turns_the_alternative_limits_on_at_any_time() {
        let s = Settings {
            alt_speed: true,
            alt_download_limit: 50,
            alt_upload_limit: 5,
            ..Settings::default()
        };
        assert_eq!(s.limits_at(3, 0), (50, 5, true));
    }

    #[test]
    fn a_daytime_schedule_covers_its_hours_on_its_days() {
        let s = scheduled(9 * 60, 17 * 60, 0b011_1110); // weekdays 9–17
        assert_eq!(s.limits_at(1, 9 * 60), (50, 5, true));
        assert!(s.limits_at(1, 17 * 60 - 1).2);
        assert_eq!(s.limits_at(1, 17 * 60), (1000, 100, false));
        assert!(!s.limits_at(1, 8 * 60 + 59).2);
        assert!(!s.limits_at(0, 12 * 60).2, "not on Sunday");
    }

    #[test]
    fn an_overnight_schedule_belongs_to_the_day_it_starts() {
        let s = scheduled(22 * 60, 6 * 60, 1 << 5); // Friday night 22:00 – Saturday 06:00
        assert!(s.limits_at(5, 23 * 60).2, "Friday evening");
        assert!(s.limits_at(6, 5 * 60).2, "Saturday early hours");
        assert!(
            !s.limits_at(6, 23 * 60).2,
            "Saturday evening isn't scheduled"
        );
        assert!(
            !s.limits_at(5, 5 * 60).2,
            "Friday early hours belong to Thursday"
        );
    }

    #[test]
    fn equal_from_and_to_means_all_day() {
        let s = scheduled(0, 0, 1 << 0);
        assert!(s.limits_at(0, 0).2 && s.limits_at(0, 1439).2);
        assert!(!s.limits_at(1, 600).2);
    }
}
