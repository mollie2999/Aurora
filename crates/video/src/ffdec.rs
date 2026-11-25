//! Linux decoding through the system's `ffmpeg` program: H.264 access units
//! (Annex B) and AAC frames go in on stdin, raw NV12 pictures and f32 PCM
//! come out on stdout. Same interface as the Media Foundation decoders in
//! `mf.rs`, so the engine threads don't care which one runs.
//!
//! Nothing is linked: without ffmpeg installed, trailers say so and the rest
//! of the app is unaffected. Our own code still downloads and demuxes.

use std::collections::BinaryHeap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

pub type Result<T> = std::result::Result<T, String>;

/// Like Media Foundation's per-thread setup: here, only "is ffmpeg there?".
pub fn init_thread() -> Result<()> {
    ffmpeg().map(|_| ()).ok_or_else(|| "FFmpeg isn't installed".into())
}

/// The `ffmpeg` to run, and whether it knows `-fps_mode` (5.1+; `-vsync`
/// before, gone in 9). `None` when it isn't installed.
fn ffmpeg() -> Option<&'static (String, bool)> {
    static FOUND: OnceLock<Option<(String, bool)>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let bin = std::env::var("STEAMINRUST_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
            let out = Command::new(&bin).args(["-hide_banner", "-version"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            let fps_mode = has_fps_mode(text.lines().next().unwrap_or_default());
            tracing::info!("video: decoding with {bin} ({})", text.lines().next().unwrap_or_default());
            Some((bin, fps_mode))
        })
        .as_ref()
}

/// "ffmpeg version n9.0.2 …", "ffmpeg version 4.4.2-0ubuntu0.22.04.1 …",
/// "ffmpeg version N-11xxxx-g… " (a git build: new).
fn has_fps_mode(version_line: &str) -> bool {
    let v = version_line.split_whitespace().nth(2).unwrap_or_default().trim_start_matches(['n', 'v']);
    let mut n = v.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    match (n.next().flatten(), n.next().flatten()) {
        (Some(major), minor) => (major, minor.unwrap_or(0)) >= (5, 1),
        (None, _) => true,
    }
}

pub fn available() -> bool {
    ffmpeg().is_some()
}

/// One running ffmpeg: we write to its stdin, a thread reads its stdout in
/// fixed-size chunks (one picture, or a block of samples).
struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Receiver<Vec<u8>>,
}

impl Proc {
    fn spawn(args: &[String], chunk: usize) -> Result<Proc> {
        let (bin, _) = ffmpeg().ok_or("FFmpeg isn't installed")?;
        let mut child = Command::new(bin)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start ffmpeg: {e}"))?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().ok_or("ffmpeg has no stdout")?;
        let (tx, rx) = sync_channel(8);
        std::thread::Builder::new()
            .name("video-ffmpeg-out".into())
            .spawn(move || {
                loop {
                    let mut buf = vec![0u8; chunk];
                    let mut got = 0;
                    while got < chunk {
                        match stdout.read(&mut buf[got..]) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => got += n,
                        }
                    }
                    if got == 0 {
                        return;
                    }
                    buf.truncate(got);
                    let last = got < chunk;
                    if tx.send(buf).is_err() || last {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Proc { child, stdin, out: rx })
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        let stdin = self.stdin.as_mut().ok_or("decoder input closed")?;
        stdin.write_all(data).map_err(|e| format!("ffmpeg: {e}"))
    }

    /// Output ready now.
    fn ready(&self, mut f: impl FnMut(Vec<u8>)) {
        loop {
            match self.out.try_recv() {
                Ok(b) => f(b),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    /// End of input: everything still inside comes out.
    fn finish(&mut self, mut f: impl FnMut(Vec<u8>)) {
        drop(self.stdin.take());
        while let Ok(b) = self.out.recv() {
            f(b);
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A decoded picture in NV12.
pub struct Picture {
    pub pts: f64,
    pub data: Vec<u8>,
    pub stride: usize,
    /// Rows in the luma plane (may be padded past the visible height).
    pub rows: usize,
}

/// Min-heap entry for presentation times (reversed: `BinaryHeap` pops the
/// largest).
#[derive(PartialEq)]
struct Pts(f64);
impl Eq for Pts {}
impl Ord for Pts {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.0.total_cmp(&self.0)
    }
}
impl PartialOrd for Pts {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

/// Access units ffmpeg may hold before we wait for a picture: frame threads
/// (`-threads`) plus up to 16 reordered frames, with some room.
const IN_FLIGHT: usize = 24;
const THREADS: &str = "4";

pub struct H264 {
    width: usize,
    height: usize,
    proc: Option<Proc>,
    /// Times of the access units sent and not yet seen come out. The decoder
    /// returns pictures in presentation order, so each one has the smallest.
    pending: BinaryHeap<Pts>,
}

impl H264 {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > crate::mp4::MAX_DIM || height > crate::mp4::MAX_DIM {
            return Err("invalid picture size".into());
        }
        ffmpeg().ok_or("FFmpeg isn't installed")?;
        // NV12 needs even sizes; ffmpeg scales to exactly this.
        Ok(Self { width: (width as usize + 1) & !1, height: (height as usize + 1) & !1, proc: None, pending: BinaryHeap::new() })
    }

    fn start(&mut self) -> Result<&mut Proc> {
        if self.proc.is_none() {
            let (_, fps_mode) = ffmpeg().ok_or("FFmpeg isn't installed")?;
            let sync = if *fps_mode { "-fps_mode" } else { "-vsync" };
            let args: Vec<String> = [
                "-probesize", "32", "-analyzeduration", "0", "-threads", THREADS, "-f", "h264", "-i", "pipe:0",
                "-an", sync, "passthrough", "-s",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain([format!("{}x{}", self.width, self.height)])
            .chain(["-pix_fmt", "nv12", "-f", "rawvideo", "-flush_packets", "1", "pipe:1"].iter().map(|s| s.to_string()))
            .collect();
            self.proc = Some(Proc::spawn(&args, self.width * self.height * 3 / 2)?);
        }
        Ok(self.proc.as_mut().unwrap())
    }

    fn picture(&mut self, data: Vec<u8>) -> Option<Picture> {
        let pts = self.pending.pop()?.0;
        (data.len() == self.width * self.height * 3 / 2).then_some(Picture { pts, data, stride: self.width, rows: self.height })
    }

    /// Decode one access unit (Annex B, SPS/PPS before key frames).
    pub fn decode(&mut self, au: &[u8], pts: f64, _dur: f64, out: &mut Vec<Picture>) -> Result<()> {
        self.pending.push(Pts(pts));
        let r = self.start().and_then(|p| p.write(au));
        if r.is_err() {
            self.flush();
            return r;
        }
        let mut got = Vec::new();
        let p = self.proc.as_ref().unwrap();
        p.ready(|b| got.push(b));
        // Keep pace with ffmpeg: past its delay (threads + B-frame
        // reordering), wait a little for each picture so they come out
        // while a segment decodes, not all at its end. Never forever: a
        // stream that holds more frames just runs a bit later.
        let mut in_flight = self.pending.len().saturating_sub(got.len());
        while in_flight > IN_FLIGHT {
            match p.out.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(b) => {
                    got.push(b);
                    in_flight -= 1;
                }
                Err(_) => break,
            }
        }
        out.extend(got.into_iter().filter_map(|b| self.picture(b)));
        Ok(())
    }

    /// End of stream: the last pictures.
    pub fn drain(&mut self, out: &mut Vec<Picture>) -> Result<()> {
        let Some(mut p) = self.proc.take() else { return Ok(()) };
        let mut got = Vec::new();
        p.finish(|b| got.push(b));
        out.extend(got.into_iter().filter_map(|b| self.picture(b)));
        self.pending.clear();
        Ok(())
    }

    /// Forget everything (a seek): the next access unit is a key frame.
    pub fn flush(&mut self) {
        self.proc = None;
        self.pending.clear();
    }
}

pub struct Aac {
    pub rate: u32,
    pub channels: u16,
    /// ADTS header fields from the AudioSpecificConfig.
    profile: u8,
    freq_index: u8,
    chan_config: u8,
    proc: Option<Proc>,
    /// Time of the next sample to come out.
    next: Option<f64>,
    carry: Vec<u8>,
}

/// AAC sample rates by ADTS frequency index.
const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

impl Aac {
    /// `config` is the AudioSpecificConfig from the MP4's esds box.
    pub fn new(config: &[u8], rate: u32, channels: u16) -> Result<Self> {
        ffmpeg().ok_or("FFmpeg isn't installed")?;
        if config.len() < 2 {
            return Err("no AAC config".into());
        }
        let object = config[0] >> 3;
        let mut freq_index = ((config[0] & 7) << 1) | (config[1] >> 7);
        let chan_config = (config[1] >> 3) & 0xF;
        if freq_index == 15 {
            // An explicit rate: ADTS can't carry it, use the nearest index.
            freq_index = RATES.iter().position(|&r| r == rate).unwrap_or(4) as u8;
        }
        // ADTS profile = object type - 1 (AAC-LC = 2 -> 1). HE-AAC streams
        // are signalled as LC with SBR inside, which ffmpeg finds.
        let profile = if matches!(object, 5 | 29) { 1 } else { object.clamp(1, 4) - 1 };
        let rate = if rate > 0 { rate } else { RATES.get(freq_index as usize).copied().unwrap_or(44100) };
        let channels = if chan_config > 0 { chan_config.min(2) as u16 } else { channels.clamp(1, 2) };
        Ok(Self { rate, channels, profile, freq_index, chan_config: chan_config.max(1), proc: None, next: None, carry: Vec::new() })
    }

    fn adts(&self, len: usize) -> [u8; 7] {
        let full = len + 7;
        [
            0xFF,
            0xF1,
            (self.profile << 6) | (self.freq_index << 2) | (self.chan_config >> 2),
            ((self.chan_config & 3) << 6) | ((full >> 11) & 3) as u8,
            ((full >> 3) & 0xFF) as u8,
            (((full & 7) << 5) as u8) | 0x1F,
            0xFC,
        ]
    }

    fn start(&mut self) -> Result<&mut Proc> {
        if self.proc.is_none() {
            let args: Vec<String> = ["-probesize", "32", "-analyzeduration", "0", "-f", "aac", "-i", "pipe:0", "-vn", "-ac"]
                .iter()
                .map(|s| s.to_string())
                .chain([self.channels.to_string(), "-ar".into(), self.rate.to_string()])
                .chain(["-f", "f32le", "-flush_packets", "1", "pipe:1"].iter().map(|s| s.to_string()))
                .collect();
            // About one AAC frame per chunk.
            self.proc = Some(Proc::spawn(&args, 1024 * self.channels as usize * 4)?);
        }
        Ok(self.proc.as_mut().unwrap())
    }

    fn samples(&mut self, data: Vec<u8>, out: &mut Vec<(f64, Vec<f32>)>) {
        self.carry.extend(data);
        let whole = self.carry.len() / 4 * 4;
        let pcm: Vec<f32> = self.carry[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        self.carry.drain(..whole);
        if pcm.is_empty() {
            return;
        }
        let pts = self.next.unwrap_or(0.0);
        self.next = Some(pts + (pcm.len() / self.channels as usize) as f64 / self.rate as f64);
        out.push((pts, pcm));
    }

    /// Decode one raw AAC frame; `out` gets (time, interleaved samples).
    pub fn decode(&mut self, frame: &[u8], pts: f64, _dur: f64, out: &mut Vec<(f64, Vec<f32>)>) -> Result<()> {
        if self.next.is_none() {
            self.next = Some(pts);
        }
        let mut buf = Vec::with_capacity(frame.len() + 7);
        buf.extend_from_slice(&self.adts(frame.len()));
        buf.extend_from_slice(frame);
        let r = self.start().and_then(|p| p.write(&buf));
        if r.is_err() {
            self.flush();
            return r;
        }
        self.ready(out);
        Ok(())
    }

    /// Samples decoded since the last call (ffmpeg works on its own).
    pub fn ready(&mut self, out: &mut Vec<(f64, Vec<f32>)>) {
        let mut got = Vec::new();
        if let Some(p) = self.proc.as_ref() {
            p.ready(|b| got.push(b));
        }
        for b in got {
            self.samples(b, out);
        }
    }

    pub fn flush(&mut self) {
        self.proc = None;
        self.next = None;
        self.carry.clear();
    }

    pub fn drain(&mut self, out: &mut Vec<(f64, Vec<f32>)>) -> Result<()> {
        let Some(mut p) = self.proc.take() else { return Ok(()) };
        let mut got = Vec::new();
        p.finish(|b| got.push(b));
        for b in got {
            self.samples(b, out);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ffmpeg_versions() {
        assert!(has_fps_mode("ffmpeg version n9.0.2 Copyright (c) 2000-2026"));
        assert!(has_fps_mode("ffmpeg version 7.1.1 Copyright"));
        assert!(has_fps_mode("ffmpeg version 5.1.6-0+deb12u1"));
        assert!(!has_fps_mode("ffmpeg version 4.4.2-0ubuntu0.22.04.1 Copyright"));
        assert!(!has_fps_mode("ffmpeg version 5.0.1"));
        assert!(has_fps_mode("ffmpeg version N-118000-gabcdef Copyright"));
    }

    #[test]
    fn rejects_oversized_dimensions() {
        assert!(H264::new(8192, 4096).is_err());
        assert!(H264::new(4096, 4097).is_err());
    }

    /// A real clip (High profile with B-frames, AAC-LC), demuxed by our own
    /// mp4.rs and decoded like the engine does. Skipped without ffmpeg.
    #[test]
    fn decodes_h264_and_aac_like_the_engine() {
        use crate::mp4::{self, Codec};
        if !available() {
            eprintln!("no ffmpeg: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("ffdec-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        let ok = Command::new(&ffmpeg().unwrap().0)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
            .args(["-c:v", "libx264", "-profile:v", "high", "-bf", "3", "-g", "30", "-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-shortest"])
            .arg(&file)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("ffmpeg can't encode the test clip (no libx264?): skipped");
            return;
        }
        let data = std::fs::read(&file).unwrap();
        let movie = mp4::parse_movie(&data).unwrap();

        let t = movie.video().unwrap();
        let Codec::H264 { nal_len, sps, pps } = &t.codec else { panic!("not H.264") };
        let mut d = H264::new(t.width, t.height).unwrap();
        let (mut au, mut pics) = (Vec::new(), Vec::new());
        for s in &t.samples {
            mp4::annex_b(&data[s.offset..s.offset + s.size], *nal_len, s.key.then_some((sps.as_slice(), pps.as_slice())), &mut au);
            d.decode(&au, t.pts(s), t.seconds(s.duration as i64), &mut pics).unwrap();
        }
        let streamed = pics.len();
        d.drain(&mut pics).unwrap();
        assert!(streamed > t.samples.len() / 2, "pictures come out while decoding ({streamed} before the end)");
        assert_eq!(pics.len(), t.samples.len(), "one picture per access unit");
        // Each picture has the time of its own frame: same bytes as ffmpeg
        // decoding the whole file in presentation order.
        let reference = Command::new(&ffmpeg().unwrap().0)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&file)
            .args(["-an", "-pix_fmt", "nv12", "-f", "rawvideo", "pipe:1"])
            .output()
            .unwrap()
            .stdout;
        let size = 320 * 180 * 3 / 2;
        assert_eq!(reference.len(), size * pics.len());
        for (i, p) in pics.iter().enumerate() {
            assert!(p.data == reference[i * size..(i + 1) * size], "picture {i} (pts {}) isn't frame {i}", p.pts);
        }
        assert!(pics.windows(2).all(|w| w[0].pts < w[1].pts), "presentation order: {:?}", pics.iter().map(|p| (p.pts * 30.0).round() as i32).collect::<Vec<_>>());
        assert!(pics.iter().all(|p| p.stride == 320 && p.rows == 180 && p.data.len() == 320 * 180 * 3 / 2));
        // testsrc2 isn't flat grey: the decoder really produced pictures.
        assert!(pics[10].data[..320 * 180].iter().any(|&y| y != pics[10].data[0]));

        // Seek-like flush, then decode again from a key frame.
        d.flush();
        let key = t.samples.iter().rposition(|s| s.key).unwrap();
        pics.clear();
        for s in &t.samples[key..] {
            mp4::annex_b(&data[s.offset..s.offset + s.size], *nal_len, s.key.then_some((sps.as_slice(), pps.as_slice())), &mut au);
            d.decode(&au, t.pts(s), 0.0, &mut pics).unwrap();
        }
        d.drain(&mut pics).unwrap();
        assert_eq!(pics.len(), t.samples.len() - key);

        let a = movie.audio().unwrap();
        let Codec::Aac { config, channels, rate } = &a.codec else { panic!("not AAC") };
        let mut aac = Aac::new(config, *rate, *channels).unwrap();
        assert_eq!((aac.rate, aac.channels), (48000, 2));
        let mut chunks = Vec::new();
        for s in &a.samples {
            aac.decode(&data[s.offset..s.offset + s.size], a.pts(s), a.seconds(s.duration as i64), &mut chunks).unwrap();
        }
        aac.drain(&mut chunks).unwrap();
        let frames: usize = chunks.iter().map(|(_, b)| b.len() / 2).sum();
        assert_eq!(frames, a.samples.len() * 1024, "every AAC frame decoded");
        assert!(chunks.windows(2).all(|w| w[0].0 < w[1].0));
        let peak = chunks.iter().flat_map(|(_, b)| b.iter()).fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05, "a sine, not silence (peak {peak})");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
