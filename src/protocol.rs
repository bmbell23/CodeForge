//! Framed messages between the CodeForge client and the persistent server.
//!
//! The server owns all state (PTYs, windows, vt100 screens) and outlives any
//! client, so a disconnect/SSH drop doesn't lose the session. The client is
//! thin: it forwards keystrokes and its terminal size, and paints whatever
//! bytes the server sends. Framing is `[tag u8][len u32-le][payload]`.

use std::io::{self, Read, Write};

/// client -> server: attach with the terminal size. Payload: rows,cols (u16 le).
pub const ATTACH: u8 = 1;
/// client -> server: raw input bytes. Payload: the bytes.
pub const INPUT: u8 = 2;
/// client -> server: the terminal was resized. Payload: rows,cols (u16 le).
pub const RESIZE: u8 = 3;
/// both ways: detach. client->server "detach me"; server->client "you're detached".
pub const DETACH: u8 = 4;
/// server -> client: terminal bytes to write to stdout. Payload: the bytes.
pub const OUTPUT: u8 = 5;

/// Write one framed message.
pub fn write_frame(w: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    let len = (payload.len() as u32).to_le_bytes();
    w.write_all(&[tag])?;
    w.write_all(&len)?;
    w.write_all(payload)?;
    w.flush()
}

/// Read one framed message: `(tag, payload)`.
pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut hdr = [0u8; 5];
    r.read_exact(&mut hdr)?;
    let tag = hdr[0];
    let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((tag, payload))
}

/// Encode a `(rows, cols)` size payload.
pub fn size_payload(rows: u16, cols: u16) -> [u8; 4] {
    let r = rows.to_le_bytes();
    let c = cols.to_le_bytes();
    [r[0], r[1], c[0], c[1]]
}

/// Decode a `(rows, cols)` size payload.
pub fn parse_size(p: &[u8]) -> Option<(u16, u16)> {
    if p.len() < 4 {
        return None;
    }
    Some((
        u16::from_le_bytes([p[0], p[1]]),
        u16::from_le_bytes([p[2], p[3]]),
    ))
}
