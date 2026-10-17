// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::{DispatchCertainty as D, Error, ErrorKind as K, Limits};
use serde_json::Value;

pub(crate) fn messages(body: &[u8], limits: &Limits, complete: bool) -> Result<Vec<Value>, Error> {
    let text =
        std::str::from_utf8(body).map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?;
    let normalized = text.replace("\r\n", "\n");
    let mut messages = Vec::new();
    let mut frames = normalized.split("\n\n").peekable();
    while let Some(frame) = frames.next() {
        if frames.peek().is_none() && !complete {
            break;
        }
        if frame.len() > limits.sse_frame_bytes {
            return Err(Error::new(K::Limit, D::ResponseReceived));
        }
        let data: Vec<_> = frame
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|s| s.strip_prefix(' ').unwrap_or(s))
            })
            .collect();
        if data.is_empty() {
            continue;
        }
        if messages.len() > limits.notifications {
            return Err(Error::new(K::Limit, D::ResponseReceived));
        }
        messages.push(
            serde_json::from_str(&data.join("\n"))
                .map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?,
        );
    }
    Ok(messages)
}

pub(crate) fn complete_prefix(body: &[u8]) -> Option<usize> {
    body.windows(2)
        .enumerate()
        .filter_map(|(i, w)| (w == b"\n\n").then_some(i + 2))
        .chain(
            body.windows(4)
                .enumerate()
                .filter_map(|(i, w)| (w == b"\r\n\r\n").then_some(i + 4)),
        )
        .max()
}
