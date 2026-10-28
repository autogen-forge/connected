//! Framing: `u32` big-endian payload length followed by a postcard-encoded [`Message`].

use std::io::{Read, Write};

use crate::message::Message;

/// Upper bound for one frame's payload. Larger transfers are split into chunks.
pub const MAX_FRAME_LEN: usize = 1 << 20;

const HEADER_LEN: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN} byte limit")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Encodes `msg` as a complete frame, header included.
pub fn encode(msg: &Message) -> Result<Vec<u8>, CodecError> {
    let payload = postcard::to_stdvec(msg)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(CodecError::TooLarge(payload.len()));
    }
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decodes the first frame in `buf`.
///
/// Returns the message and the number of bytes consumed, or `None` if `buf` does not yet
/// hold a whole frame.
pub fn try_decode(buf: &[u8]) -> Result<Option<(Message, usize)>, CodecError> {
    let Some(header) = buf.first_chunk::<HEADER_LEN>() else {
        return Ok(None);
    };
    let len = u32::from_be_bytes(*header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(CodecError::TooLarge(len));
    }
    let end = HEADER_LEN + len;
    if buf.len() < end {
        return Ok(None);
    }
    let msg = postcard::from_bytes(&buf[HEADER_LEN..end])?;
    Ok(Some((msg, end)))
}

pub fn write_message<W: Write>(w: &mut W, msg: &Message) -> Result<(), CodecError> {
    w.write_all(&encode(msg)?)?;
    Ok(())
}

pub fn read_message<R: Read>(r: &mut R) -> Result<Message, CodecError> {
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(CodecError::TooLarge(len));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(postcard::from_bytes(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::message::*;

    fn samples() -> Vec<Message> {
        vec![
            Message::Hello { version: 1, name: "mac".into() },
            Message::Enter { x: 0, y: 720, seq: 7, modifiers: Modifiers(0x22) },
            Message::Leave,
            Message::MouseMove { dx: -3, dy: 12 },
            Message::MouseButton { button: MouseButton::Back, state: ButtonState::Down },
            Message::Wheel { dx: 0, dy: -120 },
            Message::Key { usage: 0x04, state: ButtonState::Up },
            Message::ClipAnnounce {
                id: 9,
                offers: vec![
                    ClipOffer { format: ClipFormat::Text, size: 5 },
                    ClipOffer { format: ClipFormat::Png, size: 1 << 20 },
                ],
            },
            Message::ClipRequest { id: 9, format: ClipFormat::Png },
            Message::ClipData {
                id: 9,
                format: ClipFormat::Text,
                offset: 0,
                data: b"hello".to_vec(),
                last: true,
            },
            Message::Ping(1),
            Message::Pong(1),
            Message::PairDecision { accepted: true },
            Message::Layout {
                screens: vec![LayoutScreen {
                    name: "desk".into(),
                    x: -10,
                    y: 20,
                    displays: vec![Rect::new(0, 0, 1920, 1080)],
                    scales: vec![200],
                    os: Os::Mac,
                    you: false,
                    online: true,
                }],
            },
            Message::Screen {
                displays: vec![Rect::new(0, 0, 2560, 1440), Rect::new(2560, 360, 1920, 1080)],
                scales: vec![100, 150],
                os: Os::Windows,
            },
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for msg in samples() {
            let frame = encode(&msg).unwrap();
            let (decoded, used) = try_decode(&frame).unwrap().unwrap();
            assert_eq!(decoded, msg);
            assert_eq!(used, frame.len());
        }
    }

    #[test]
    fn partial_frame_needs_more_bytes() {
        let frame = encode(&Message::Ping(42)).unwrap();
        for cut in 0..frame.len() {
            assert!(try_decode(&frame[..cut]).unwrap().is_none(), "cut at {cut}");
        }
    }

    #[test]
    fn back_to_back_frames_decode_one_at_a_time() {
        let mut buf = encode(&Message::Ping(1)).unwrap();
        buf.extend(encode(&Message::Leave).unwrap());

        let (first, used) = try_decode(&buf).unwrap().unwrap();
        assert_eq!(first, Message::Ping(1));
        let (second, _) = try_decode(&buf[used..]).unwrap().unwrap();
        assert_eq!(second, Message::Leave);
    }

    #[test]
    fn oversized_length_header_is_rejected() {
        let header = ((MAX_FRAME_LEN + 1) as u32).to_be_bytes();
        assert!(matches!(try_decode(&header), Err(CodecError::TooLarge(_))));
        assert!(matches!(
            read_message(&mut &header[..]),
            Err(CodecError::TooLarge(_))
        ));
    }

    #[test]
    fn oversized_message_is_not_encoded() {
        let msg = Message::ClipData {
            id: 1,
            format: ClipFormat::Png,
            offset: 0,
            data: vec![0; MAX_FRAME_LEN],
            last: false,
        };
        assert!(matches!(encode(&msg), Err(CodecError::TooLarge(_))));
    }

    #[test]
    fn io_helpers_round_trip() {
        let mut wire = Vec::new();
        for msg in samples() {
            write_message(&mut wire, &msg).unwrap();
        }
        let mut reader = &wire[..];
        for expected in samples() {
            assert_eq!(read_message(&mut reader).unwrap(), expected);
        }
    }

    #[test]
    fn garbage_payload_is_an_error() {
        let mut frame = 3u32.to_be_bytes().to_vec();
        frame.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
        assert!(matches!(try_decode(&frame), Err(CodecError::Postcard(_))));
    }
}
