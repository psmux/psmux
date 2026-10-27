// Issue #919: input-buffer-size.  A sixel DCS is held to the limit
// set_input_buffer_size records, which psmux's `input-buffer-size` server
// option sets as tmux's options.c sets the static input_input reads
// (input.c).  The limit is one per process, so this check lives in its own
// test binary: the 1 MiB tests in tests-rs/test_issue431_sixel_parse.rs run
// beside each other and assume the default.

use vt100_psmux as vt100;

fn sixel_with_body(len: usize) -> Vec<u8> {
    let mut v = b"\x1bPq#0;2;100;0;0#0".to_vec();
    v.extend(std::iter::repeat(b'~').take(len));
    v.extend_from_slice(b"\x1b\\");
    v
}

fn images_after(bytes: &[u8]) -> usize {
    let mut p = vt100::Parser::new(24, 80, 100);
    p.process(bytes);
    p.screen().images().len()
}

#[test]
fn a_sixel_is_held_to_the_input_buffer_size_set_last() {
    let two_mib = sixel_with_body(2 << 20);

    assert_eq!(vt100::input_buffer_size(), vt100::DEFAULT_INPUT_BUFFER_SIZE);
    assert_eq!(vt100::DEFAULT_INPUT_BUFFER_SIZE, 1 << 20);
    assert_eq!(images_after(&two_mib), 0, "over the default it is discarded");

    vt100::set_input_buffer_size(4 << 20);
    assert_eq!(vt100::input_buffer_size(), 4 << 20);
    assert_eq!(images_after(&two_mib), 1, "under a raised limit it is kept");
    assert_eq!(
        images_after(&sixel_with_body(5 << 20)),
        0,
        "over the raised limit it is discarded",
    );

    vt100::set_input_buffer_size(vt100::DEFAULT_INPUT_BUFFER_SIZE);
    assert_eq!(images_after(&two_mib), 0, "back at the default it is discarded again");
    assert_eq!(
        images_after(&sixel_with_body((1 << 20) - 64)),
        1,
        "under the default it is kept",
    );
}
