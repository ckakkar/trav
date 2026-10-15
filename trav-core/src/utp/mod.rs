//! µTP (BEP 29): BitTorrent over UDP with LEDBAT congestion control.
//!
//! Most clients take connections over µTP as well as TCP, and it is the only
//! transport NAT hole punching works with (see `torrent::holepunch`): two
//! peers behind NATs that drop unsolicited packets can still reach each other
//! by sending at the same moment, because each side's outgoing packet opens
//! its own NAT for the other's.

mod conn;
mod packet;
mod socket;

pub use conn::Dial;
pub use socket::{Other, UdpSender, UtpProbe, UtpSocket, UtpStream};
