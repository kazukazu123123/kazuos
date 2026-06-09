pub(crate) fn play_wav(data: &[u8]) {
    let Some(wav) = parse_wav_public(data) else {
        crate::println!("unsupported WAV");
        return;
    };
    crate::drivers::ac97::play_wav_parsed(&wav);
}

#[derive(Clone, Copy)]
pub(crate) struct WavInfo<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) channels: u16,
    pub(crate) sample_rate: u32,
    pub(crate) bits: u16,
}

impl WavInfo<'_> {
    pub(crate) fn frames(&self) -> usize {
        self.data.len() / (self.channels as usize * (self.bits as usize / 8))
    }

    pub(crate) fn fill_stereo_i16(&self, start_frame: usize, frames: usize, out: &mut [i16]) {
        let frame_size = self.channels as usize * (self.bits as usize / 8);
        let total_frames = self.frames();
        let mut i = 0usize;
        while i < frames {
            let frame = start_frame + i;
            if frame >= total_frames {
                out[i * 2] = 0;
                out[i * 2 + 1] = 0;
                i += 1;
                continue;
            }
            let base = frame * frame_size;
            let (left, right) = if self.bits == 16 {
                if self.channels == 1 {
                    let sample = i16::from_le_bytes([self.data[base], self.data[base + 1]]);
                    (sample, sample)
                } else {
                    (
                        i16::from_le_bytes([self.data[base], self.data[base + 1]]),
                        i16::from_le_bytes([self.data[base + 2], self.data[base + 3]]),
                    )
                }
            } else if self.channels == 1 {
                let sample = ((self.data[base] as i16) - 128) << 8;
                (sample, sample)
            } else {
                (
                    ((self.data[base] as i16) - 128) << 8,
                    ((self.data[base + 1] as i16) - 128) << 8,
                )
            };
            out[i * 2] = left;
            out[i * 2 + 1] = right;
            i += 1;
        }
    }
}

pub(crate) fn parse_wav_public(data: &[u8]) -> Option<WavInfo<'_>> {
    parse_wav(data)
}

fn parse_wav(data: &[u8]) -> Option<WavInfo<'_>> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data_start = 0usize;
    let mut data_len = 0usize;
    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let len = le_u32(data, offset + 4) as usize;
        offset += 8;
        if offset + len > data.len() {
            return None;
        }
        if id == b"fmt " {
            if len < 16 || le_u16(data, offset) != 1 {
                return None;
            }
            channels = le_u16(data, offset + 2);
            sample_rate = le_u32(data, offset + 4);
            bits = le_u16(data, offset + 14);
        } else if id == b"data" {
            data_start = offset;
            data_len = len;
        }
        offset += (len + 1) & !1;
    }
    if !(channels == 1 || channels == 2)
        || !(bits == 8 || bits == 16)
        || data_start == 0
        || data_len < 1
    {
        return None;
    }
    Some(WavInfo {
        data: &data[data_start..data_start + data_len],
        channels,
        sample_rate,
        bits,
    })
}

fn le_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}
