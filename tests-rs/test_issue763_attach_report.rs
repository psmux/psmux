// Real authenticated ingress, without starting a psmux server or shell.
use super::*;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, RwLock};
use crate::terminal_features::TerminalFeatures;

#[test]
fn attach_ingress_carries_optional_features_and_ignores_duplicate_attach() {
    for (metadata, expected) in [
        ("", None),
        (" features=0xf", Some(15)),
        (" features=0x0", Some(0)),
        (" features=0x000000000000000f", Some(15)),
        (" features=0x8000000000000002", Some(0x8000000000000002)),
        (" features=0xffffffffffffffff", Some(u64::MAX)),
        (" features=0x10000000000000000", None),
        (" features=", None),
        (" features=0x", None),
        (" features=0x-f", None),
        (" features=0x+f", None),
        (" features=0xg", None),
        (" features=256,RGB,title", None),
        (" future-field=value", None),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let handler = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, tx, "test763", Arc::new(RwLock::new(Default::default())));
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        stream.write_all(b"AUTH test763\n").unwrap();
        let mut reply = String::new();
        reader.read_line(&mut reply).unwrap();
        assert_eq!(reply, "OK\n");
        assert!(rx.try_recv().is_err(), "authentication must not attach a client");
        // PERSISTENT exercises the same protocol path as the frame client.
        write!(stream, "PERSISTENT\nclient-attach{metadata}\nclient-attach features=0x0\n").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        handler.join().unwrap();
        let mut reports = Vec::new();
        while let Ok(req) = rx.recv_timeout(Duration::from_millis(100)) {
            if let CtrlReq::ClientAttach(_, _, features) = req {
                reports.push(features);
            }
        }
        assert_eq!(reports, vec![expected.map(TerminalFeatures::from_bits)], "metadata {metadata:?}");
    }
}
