//! Decode bounded byte prefixes without replacing malformed input.
use std::borrow::Cow;

use encoding_rs::{DecoderResult, Encoding, UTF_16BE, UTF_16LE};

use super::ReadStatus;

pub(super) struct Decoded<'a> {
    pub text: Cow<'a, str>,
    pub encoding: &'static str,
}

type Failure = (ReadStatus, &'static str);

const INVALID_TEXT: Failure = (
    ReadStatus::UnknownEncoding,
    "文本编码无效或内容损坏；未用替换字符生成证据",
);
const BINARY: Failure = (ReadStatus::Binary, "检测到二进制/控制内容，不返回文本");

fn is_binary_byte(byte: u8) -> bool {
    byte < 0x20 && !matches!(byte, b'\t' | b'\r' | b'\n')
}

fn utf8(bytes: &[u8], prefix: bool) -> Result<&str, Failure> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if prefix && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| INVALID_TEXT)
        }
        Err(_) => Err(INVALID_TEXT),
    }
}

fn strict_decode(
    bytes: &[u8],
    prefix: bool,
    encoding: &'static Encoding,
) -> Result<Decoded<'static>, Failure> {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let capacity = decoder
        .max_utf8_buffer_length_without_replacement(bytes.len())
        .ok_or(INVALID_TEXT)?;
    let mut text = String::with_capacity(capacity);
    // When the byte budget cuts a multibyte character, keep the valid prefix;
    // the caller will not publish the clipped final line as evidence.
    if decoder
        .decode_to_string_without_replacement(bytes, &mut text, !prefix)
        .0
        != DecoderResult::InputEmpty
    {
        return Err(INVALID_TEXT);
    }
    if text.bytes().any(is_binary_byte) {
        return Err(BINARY);
    }
    Ok(Decoded {
        text: Cow::Owned(text),
        encoding: encoding.name(),
    })
}

pub(super) fn decode<'a>(
    bytes: &'a [u8],
    prefix: bool,
    fallback: Option<&'static Encoding>,
) -> Result<Decoded<'a>, Failure> {
    if bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
        return Err((
            ReadStatus::UnknownEncoding,
            "检测到 UTF-32 BOM，暂不支持此编码",
        ));
    }
    if let Some(body) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return strict_decode(body, prefix, UTF_16LE);
    }
    if let Some(body) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return strict_decode(body, prefix, UTF_16BE);
    }
    if bytes.iter().copied().any(is_binary_byte) {
        return Err(BINARY);
    }
    if let Some(body) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return Ok(Decoded {
            text: Cow::Borrowed(utf8(body, prefix)?),
            encoding: "UTF-8",
        });
    }
    match utf8(bytes, prefix) {
        Ok(text) => Ok(Decoded {
            text: Cow::Borrowed(text),
            encoding: "UTF-8",
        }),
        Err(error) => match fallback {
            Some(encoding) => strict_decode(bytes, prefix, encoding),
            None => Err(error),
        },
    }
}

pub(super) fn physical_lines(mut text: &str) -> impl Iterator<Item = (&str, bool)> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        let Some(end) = text.find(['\r', '\n']) else {
            return Some((std::mem::take(&mut text), false));
        };
        let (line, rest) = text.split_at(end);
        text = rest.strip_prefix("\r\n").unwrap_or(&rest[1..]);
        Some((line, true))
    })
}
