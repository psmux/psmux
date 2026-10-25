use super::*;
use std::net::TcpListener;

#[test]
fn real_client_handshake_advertises_the_client_mask_as_hex() {
    let expected = format!("client-attach features={:#x}", reported_terminal_features().bits());
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
    assert_eq!(server.join().unwrap(), vec![expected]);
}

#[test]
fn ansi_conemu_reports_colour_and_title_without_osc8_links() {
    let profile = [("ConEmuPID", "12345"), ("ConEmuANSI", "ON")];
    let features = terminal_features_from_env(|name| {
        profile.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    });
    assert_eq!(features.bits(), 0xb);
    assert_eq!(features.to_string(), "256,RGB,title");
}

#[test]
fn unknown_or_incomplete_profiles_keep_the_compatibility_advertisement() {
    for profile in [
        vec![],
        vec![("WT_SESSION", "terminal-session")],
        vec![("TERM", "xterm-256color"), ("COLORTERM", "truecolor")],
        vec![("ConEmuPID", "12345")],
        vec![("ConEmuANSI", "ON")],
        vec![("ConEmuPID", ""), ("ConEmuANSI", "ON")],
        vec![("ConEmuPID", "12345"), ("ConEmuANSI", "OFF")],
        vec![("ConEmuPID", "12345"), ("ConEmuANSI", "")],
        vec![("ConEmuPID", "12345"), ("ConEmuANSI", "invalid")],
    ] {
        let features = terminal_features_from_env(|name| {
            profile.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
        });
        assert_eq!(features.bits(), 0xf, "{profile:?}");
        assert_eq!(features.to_string(), "256,RGB,hyperlinks,title", "{profile:?}");
    }
}

#[test]
fn conflicting_host_or_ssh_hints_do_not_select_the_conemu_profile() {
    let conemu = [("ConEmuPID", "12345"), ("ConEmuANSI", "ON")];
    let lookup = |name: &str| {
        conemu.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    };
    assert_eq!(
        terminal_features_from_env(lookup).bits(), 0xb,
        "precondition: ANSI ConEmu is recognized",
    );
    for hint in [
        "WT_SESSION", "TERM_PROGRAM", "TERMINAL_EMULATOR", "WEZTERM_PANE",
        "ALACRITTY_LOG", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY",
    ] {
        for value in ["present", ""] {
            let features = terminal_features_from_env(|name| {
                if name == hint { Some(value.to_string()) } else { lookup(name) }
            });
            assert_eq!(features.bits(), 0xf, "{hint}={value:?}");
        }
    }
}
