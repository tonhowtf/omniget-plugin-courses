use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, Result};

pub fn vtt_to_srt(vtt_path: &Path, srt_path: &Path) -> Result<()> {
    let raw = fs::read_to_string(vtt_path)
        .map_err(|e| anyhow!("read vtt failed: {}", e))?;
    let content = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.split('\n').peekable();

    let mut found_header = false;
    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed.starts_with("WEBVTT") {
            found_header = true;
            lines.next();
            break;
        }
        if trimmed.is_empty() {
            lines.next();
            continue;
        }
        break;
    }
    if !found_header {
        return Err(anyhow!("not a WEBVTT file"));
    }

    let mut output = String::new();
    let mut index: u32 = 0;

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("NOTE")
            || trimmed.starts_with("STYLE")
            || trimmed.starts_with("REGION")
        {
            for inner in lines.by_ref() {
                if inner.trim().is_empty() {
                    break;
                }
            }
            continue;
        }

        let timestamp_line = if trimmed.contains("-->") {
            trimmed.to_string()
        } else {
            match lines.next() {
                Some(next) => {
                    let t = next.trim();
                    if !t.contains("-->") {
                        continue;
                    }
                    t.to_string()
                }
                None => break,
            }
        };

        let (start, end) = match parse_timing(&timestamp_line) {
            Some(p) => p,
            None => continue,
        };

        let mut payload_lines: Vec<String> = Vec::new();
        for p in lines.by_ref() {
            if p.trim().is_empty() {
                break;
            }
            payload_lines.push(decode_entities(p));
        }
        if payload_lines.is_empty() {
            continue;
        }

        index += 1;
        output.push_str(&format!("{}\r\n", index));
        output.push_str(&format!("{} --> {}\r\n", start, end));
        for p in &payload_lines {
            output.push_str(p);
            output.push_str("\r\n");
        }
        output.push_str("\r\n");
    }

    let mut file = fs::File::create(srt_path)
        .map_err(|e| anyhow!("create srt failed: {}", e))?;
    file.write_all(output.as_bytes())
        .map_err(|e| anyhow!("write srt failed: {}", e))?;
    Ok(())
}

fn parse_timing(line: &str) -> Option<(String, String)> {
    let mut parts = line.splitn(2, "-->");
    let left = parts.next()?.trim();
    let rest = parts.next()?.trim();
    let right = rest.split_whitespace().next()?;
    Some((normalize_timestamp(left)?, normalize_timestamp(right)?))
}

fn normalize_timestamp(ts: &str) -> Option<String> {
    let (time_part, ms_part) = ts
        .split_once('.')
        .or_else(|| ts.split_once(','))?;
    let segs: Vec<&str> = time_part.split(':').collect();
    let (h, m, s) = match segs.len() {
        3 => (segs[0], segs[1], segs[2]),
        2 => ("00", segs[0], segs[1]),
        _ => return None,
    };
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    let s: u32 = s.parse().ok()?;
    let ms: u32 = ms_part.parse().ok()?;
    Some(format!("{:02}:{:02}:{:02},{:03}", h, m, s, ms))
}

fn decode_entities(input: &str) -> String {
    let s = input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    s.replace("&amp;", "&")
}
