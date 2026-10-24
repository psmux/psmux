use super::*;
use std::net::TcpListener;

#[test]
fn real_client_handshake_advertises_the_default_mask_as_hex() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "AUTH test763-client\n");
        stream.write_all(b"OK\n").unwrap();
        let mut reports = Vec::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 { break; }
            if line.starts_with("client-attach") {
                reports.push(line.trim_end().to_string());
                break;
            }
        }
        reports
    });
    let (_stream, _frames) = establish_connection_with_timeout(
        &addr.to_string(), "test763-client", Duration::from_secs(2),
    ).unwrap();
    assert_eq!(server.join().unwrap(), vec!["client-attach features=0xf"]);
}
