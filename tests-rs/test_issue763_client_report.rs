use super::*;
use std::net::TcpListener;

#[test]
fn real_client_handshake_advertises_the_client_mask_as_hex() {
    let expected = format!("client-attach features={:#x}", reported_terminal_features(
        |name| std::env::var(name).ok(), crate::platform::is_classic_console_output,
    ).bits());
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

fn profile_features(profile: &[(&str, &str)], classic: bool) -> crate::terminal_features::TerminalFeatures {
    reported_terminal_features(
        |name| profile.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()),
        || classic,
    )
}

#[test]
fn terminal_identity_keeps_matching_hints_together_and_conflicts_unknown() {
    use ClientTerminal::*;
    for (profile, expected) in [
        (vec![("WT_SESSION", "session"), ("WT_PROFILE_ID", "profile")], WindowsTerminal),
        (vec![("WT_PROFILE_ID", "profile")], WindowsTerminal),
        (vec![("TERM_PROGRAM", "WezTerm"), ("TERM_PROGRAM_VERSION", "20240203-110809-5046fc22"), ("WEZTERM_PANE", "0")], WezTerm),
        (vec![("TERM_PROGRAM", "WezTerm")], WezTerm),
        (vec![("WEZTERM_PANE", "0")], WezTerm),
        (vec![("ALACRITTY_LOG", "Alacritty-123.log")], Alacritty),
        (vec![("ConEmuPID", "12345"), ("ConEmuANSI", "ON")], AnsiConEmu),
        (vec![("ConEmuPID", "12345")], Unknown),
        (vec![("ConEmuPID", ""), ("ConEmuANSI", "ON")], Unknown),
        (vec![("WT_SESSION", "")], Unknown),
        (vec![("TERM_PROGRAM_VERSION", "1.0")], Unknown),
        (vec![("TERM_PROGRAM", "unrecognized-terminal")], Unknown),
        (vec![("WT_SESSION", "session"), ("WEZTERM_PANE", "0")], Unknown),
        (vec![("WEZTERM_PANE", "0"), ("ALACRITTY_LOG", "Alacritty-123.log")], Unknown),
        (vec![("WT_SESSION", "session"), ("TERM_PROGRAM", "unrecognized-terminal")], Unknown),
        (vec![("WT_SESSION", "session"), ("SSH_TTY", "")], Unknown),
    ] {
        let terminal = identify_client_terminal(
            |name| profile.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()),
            || panic!("host hints must resolve without querying the console"),
        );
        assert_eq!(terminal, expected, "{profile:?}");
    }
    assert_eq!(identify_client_terminal(|_| None, || false), Unknown);
    assert_eq!(identify_client_terminal(|_| None, || true), ClassicConsole);
}

#[test]
fn each_feature_owns_its_terminal_policy() {
    use ClientTerminal::*;
    for (terminal, hyperlinks) in [
        (WindowsTerminal, true), (WezTerm, true), (Alacritty, true),
        (AnsiConEmu, false), (ClassicConsole, false), (Unknown, true),
    ] {
        assert!(advertise_color_256(terminal), "{terminal:?}");
        assert!(advertise_rgb(terminal), "{terminal:?}");
        assert_eq!(advertise_hyperlinks(terminal), hyperlinks, "{terminal:?}");
        assert!(advertise_title(terminal), "{terminal:?}");
    }
}

#[test]
fn a_report_observes_the_classic_console_once() {
    let observations = std::cell::Cell::new(0);
    assert_eq!(observations.get(), 0);
    let features = reported_terminal_features(|_| None, || {
        observations.set(observations.get() + 1);
        true
    });
    assert_eq!(features.bits(), 0xb);
    assert_eq!(observations.get(), 1);
}

#[test]
fn ansi_conemu_reports_colour_and_title_without_osc8_links() {
    let profile = [("ConEmuPID", "12345"), ("ConEmuANSI", "ON")];
    for classic in [false, true] {
        let features = profile_features(&profile, classic);
        assert_eq!(features.bits(), 0xb);
        assert_eq!(features.to_string(), "256,RGB,title");
    }
}

#[test]
fn classic_console_requires_positive_identification_not_missing_hints() {
    assert_eq!(profile_features(&[], false).bits(), 0xf, "precondition: no classic evidence");
    let features = profile_features(&[], true);
    assert_eq!(features.bits(), 0xb);
    assert_eq!(features.to_string(), "256,RGB,title");
}

#[test]
fn modern_terminal_profiles_keep_all_four_features_even_with_classic_evidence() {
    for profile in [
        vec![("WT_SESSION", "terminal-session"), ("WT_PROFILE_ID", "profile-id")],
        vec![("TERM_PROGRAM", "WezTerm"), ("TERM_PROGRAM_VERSION", "20240203-110809-5046fc22"), ("WEZTERM_PANE", "0")],
        vec![("ALACRITTY_LOG", r"C:\Temp\Alacritty-123.log"), ("TERM", "alacritty"), ("COLORTERM", "truecolor")],
    ] {
        for classic in [false, true] {
            let features = profile_features(&profile, classic);
            assert_eq!(features.bits(), 0xf, "{profile:?} classic={classic}");
            assert_eq!(features.to_string(), "256,RGB,hyperlinks,title", "{profile:?}");
        }
    }
}

#[test]
fn incomplete_conemu_profiles_keep_the_compatibility_advertisement() {
    for profile in [
        vec![("ConEmuPID", "12345")],
        vec![("ConEmuANSI", "ON")],
        vec![("ConEmuPID", ""), ("ConEmuANSI", "ON")],
        vec![("ConEmuPID", "12345"), ("ConEmuANSI", "OFF")],
        vec![("ConEmuPID", "12345"), ("ConEmuANSI", "")],
        vec![("ConEmuPID", "12345"), ("ConEmuANSI", "invalid")],
    ] {
        for classic in [false, true] {
            assert_eq!(profile_features(&profile, classic).bits(), 0xf, "{profile:?} classic={classic}");
        }
    }
}

#[test]
fn competing_or_partial_host_hints_never_select_a_restrictive_profile() {
    let conemu = [("ConEmuPID", "12345"), ("ConEmuANSI", "ON")];
    assert_eq!(profile_features(&conemu, false).bits(), 0xb, "precondition: ANSI ConEmu is recognized");
    assert_eq!(profile_features(&[], true).bits(), 0xb, "precondition: classic console is recognized");
    for hint in [
        "WT_SESSION", "WT_PROFILE_ID", "TERM_PROGRAM", "TERM_PROGRAM_VERSION",
        "TERMINAL_EMULATOR", "WEZTERM_PANE", "ALACRITTY_LOG", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY",
    ] {
        for value in ["present", ""] {
            for profile in [vec![], conemu.to_vec()] {
                let features = reported_terminal_features(
                    |name| if name == hint { Some(value.to_string()) }
                        else { profile.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()) },
                    || panic!("ambiguous profile must not query the console"),
                );
                assert_eq!(features.bits(), 0xf, "{hint}={value:?} profile={profile:?}");
            }
        }
    }
}
