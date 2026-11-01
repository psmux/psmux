//! Run-shell completion text and opt-in native CLI wire framing.

// AUTH runs before command aliases or application dispatch.
pub(crate) const AUTH_CAPABILITY: &str = "PSMUX_RUN_SHELL_NATIVE_V1";
pub(crate) const NEGOTIATION_ACK: &str = "\u{0001}PSMUX_RUN_SHELL_NATIVE_V1\u{0001}\n";

pub(crate) fn negotiate(reader: &mut impl std::io::BufRead, writer: &mut impl std::io::Write, key: &str) -> std::io::Result<()> {
    writeln!(writer, "AUTH {} {}", key, AUTH_CAPABILITY)?;
    writer.flush()?;
    let mut reply = String::new();
    reader.read_line(&mut reply)?;
    if !reply.ends_with('\n') {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "incomplete run-shell negotiation"));
    }
    match reply.as_str() {
        NEGOTIATION_ACK => Ok(()),
        "OK\n" | "OK\r\n" => Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "server does not support native run-shell replies")),
        _ => Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, reply.trim().to_string())),
    }
}

// This is only the native wire payload, not a job or a UI presentation.
#[derive(Debug, PartialEq)]
pub(crate) struct CliReply {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) fn write_reply(writer: &mut impl std::io::Write, code: i32, stdout: &[u8], stderr: &[u8]) -> std::io::Result<()> {
    writeln!(writer, "RS1 {} {} {}", code, stdout.len(), stderr.len())?;
    writer.write_all(stdout)?;
    writer.write_all(stderr)?;
    writer.flush()
}

pub(crate) fn read_reply(reader: &mut impl std::io::BufRead) -> std::io::Result<CliReply> {
    use std::io::{BufRead, Read};
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid run-shell reply");
    let mut header = String::new();
    (&mut *reader).take(128).read_line(&mut header)?;
    if !header.ends_with('\n') { return Err(invalid()); }
    let fields: Vec<_> = header.split_whitespace().collect();
    if fields.len() != 4 || fields[0] != "RS1" { return Err(invalid()); }
    let code = fields[1].parse().map_err(|_| invalid())?;
    let stdout_len: u64 = fields[2].parse().map_err(|_| invalid())?;
    let stderr_len: u64 = fields[3].parse().map_err(|_| invalid())?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    // Read incrementally rather than allocate untrusted advertised lengths.
    (&mut *reader).take(stdout_len).read_to_end(&mut stdout)?;
    (&mut *reader).take(stderr_len).read_to_end(&mut stderr)?;
    if stdout.len() as u64 != stdout_len || stderr.len() as u64 != stderr_len {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "incomplete run-shell reply"));
    }
    let mut extra = [0];
    if reader.read(&mut extra)? != 0 { return Err(invalid()); }
    Ok(CliReply { code, stdout, stderr })
}

pub(crate) fn output_text(command: &str, out: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&out.stderr);
    if !stderr_text.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr_text);
    }
    let status = match out.status.code() {
        Some(0) => None,
        Some(code) => Some(format!("'{}' returned {}", command, code)),
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                out.status.signal().map(|signal| format!("'{}' terminated by signal {}", command, signal))
            }
            #[cfg(not(unix))]
            { None }
        }
    };
    if let Some(status) = status {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&status);
        text.push('\n');
    }
    text
}

#[cfg(test)]
#[path = "../tests-rs/test_run_shell_output.rs"]
mod tests;
