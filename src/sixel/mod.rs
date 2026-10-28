//! Pane sixel images on the client: whether the terminal draws them and how
//! a frame writes them.  The pane side (parsing, layout, storage) is in
//! `vt100::image` and `vt100::perform`, the server side in
//! `server::helpers` (`visible_pane_images`, `visible_image_blobs`).

pub(crate) mod draw;
pub(crate) mod host;
