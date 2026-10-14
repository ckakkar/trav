//! A TCP connection's round-trip time, from the kernel's own estimate.

use std::os::fd::RawFd;

/// Smoothed round-trip time of a connected TCP socket, in milliseconds.
#[cfg(target_os = "macos")]
pub fn srtt_ms(fd: RawFd) -> Option<u32> {
    // SAFETY: an all-zero tcp_connection_info is valid, getsockopt writes at
    // most `len` bytes into it, and `fd` is a socket the caller keeps open.
    let mut info: libc::tcp_connection_info = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::tcp_connection_info>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_CONNECTION_INFO,
            (&mut info as *mut libc::tcp_connection_info).cast(),
            &mut len,
        )
    };
    (rc == 0 && info.tcpi_srtt > 0).then_some(info.tcpi_srtt)
}

#[cfg(not(target_os = "macos"))]
pub fn srtt_ms(_fd: RawFd) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    #[tokio::test]
    async fn reads_a_live_connection() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (client, server) = tokio::join!(tokio::net::TcpStream::connect(addr), l.accept());
        let (mut client, mut server) = (client.unwrap(), server.unwrap().0);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // Exchange a little data so the kernel has RTT samples.
        for _ in 0..20 {
            client.write_all(&[0u8; 512]).await.unwrap();
            let mut b = [0u8; 512];
            server.read_exact(&mut b).await.unwrap();
            server.write_all(&b).await.unwrap();
            client.read_exact(&mut b).await.unwrap();
        }
        let rtt = super::srtt_ms(client.as_raw_fd());
        if cfg!(target_os = "macos") {
            assert!(rtt.is_some_and(|ms| ms < 1000), "{rtt:?}");
        }
    }
}
