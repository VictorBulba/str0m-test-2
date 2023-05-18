use std::collections::VecDeque;
use std::time::{Duration, Instant};
use str0m::Bitrate;

struct BitrateSample {
    data_size: u32,
    time: Instant,
}

struct BitrateMeasure {
    samples: VecDeque<BitrateSample>,
    samples_count: usize,
}

impl BitrateMeasure {
    fn new(samples_count: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(samples_count),
            samples_count,
        }
    }

    fn push(&mut self, data_size: u32) {
        if self.samples.len() == self.samples_count {
            self.samples.pop_front();
        }
        self.samples.push_back(BitrateSample {
            data_size,
            time: Instant::now(),
        });
    }

    fn bitrate(&self) -> Bitrate {
        let since_earliest = match self.samples.get(0) {
            Some(sample) => sample.time.elapsed().as_secs_f64(),
            None => return Bitrate::ZERO,
        };
        let sum: u32 = self.samples.iter().map(|s| s.data_size).sum();
        let byterate = (sum as f64) / since_earliest;
        Bitrate::from(byterate * 8.0)
    }
}

pub(crate) struct EncodedFrame {
    data: Vec<u8>,
    duration: Duration,
    current_bitrate: Bitrate,
    time: Instant,
}

impl EncodedFrame {
    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }

    pub(crate) fn duration(&self) -> Duration {
        self.duration
    }

    pub(crate) fn current_bitrate(&self) -> Bitrate {
        self.current_bitrate
    }

    pub(crate) fn time(&self) -> Instant {
        self.time
    }
}

impl std::fmt::Debug for EncodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedFrame")
            .field("data_size", &self.data().len())
            .field("duration", &self.duration)
            .field("current_bitrate", &self.current_bitrate)
            .finish()
    }
}

struct CurrentBitrate {
    estimations: VecDeque<Bitrate>,
    bitrate: Bitrate,
    last_update: Instant,
}

impl CurrentBitrate {
    fn new() -> Self {
        const INITIAL_BITRATE: Bitrate = Bitrate::mbps(10);
        let mut estimations = VecDeque::with_capacity(60);
        estimations.push_back(INITIAL_BITRATE);
        Self {
            estimations,
            bitrate: INITIAL_BITRATE,
            last_update: Instant::now(),
        }
    }

    fn average_estimated(&self) -> Bitrate {
        let mut sum = 0;
        for sample in self.estimations.iter() {
            sum += sample.as_u64();
        }
        Bitrate::from(sum / self.estimations.len() as u64)
    }

    fn update(&mut self, estimation: Bitrate) -> bool {
        if self.estimations.len() == self.estimations.capacity() {
            self.estimations.pop_front();
        }
        self.estimations.push_back(estimation);

        let new = self.average_estimated();

        let ratio = if new > self.bitrate {
            new.as_f64() / self.bitrate.as_f64()
        } else {
            self.bitrate.as_f64() / new.as_f64()
        };
        let update_now = ratio >= 2.0 || self.last_update.elapsed() > Duration::from_secs(3);
        if update_now {
            self.bitrate = new;
            self.last_update = Instant::now();
        }
        update_now
    }
}

pub(crate) struct Encoder {
    encoder: vpx_encode::Encoder,
    width: u32,
    height: u32,
    stat: BitrateMeasure,
    current: CurrentBitrate,
    pts: u64,
}

unsafe impl Send for Encoder {}

impl Encoder {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        // let mut encoder = ffmpeg_next::encoder::new().video().unwrap();
        // encoder.set_width(width);
        // encoder.set_height(height);
        // encoder.set_format(Pixel::from(AV_PIX_FMT_0BGR32));
        // encoder.set_time_base(1.0 / 60.0);

        let config = vpx_encode::Config {
            width,
            height,
            timebase: [1, 60],
            bitrate: 5 * 1024 * 1024,
            codec: vpx_encode::VideoCodecId::VP8,
        };
        let encoder = vpx_encode::Encoder::new(config).unwrap();

        // let mut options = ffmpeg_next::util::dictionary::Owned::new();
        // options.set("preset", "p1");
        // options.set("rc", "vbr");
        // options.set("zerolatency", "1");
        // options.set("tune", "ull");
        // let encoder = encoder.open_as_with("h264_nvenc", options).unwrap();
        Self {
            encoder,
            width,
            height,
            stat: BitrateMeasure::new(60 * 2),
            current,
            pts: 0,
        }
    }

    pub(crate) fn encode(
        &mut self,
        data: &[u8],
        duration: Duration,
        estimated_bitrate: Bitrate,
        time: Instant,
    ) -> EncodedFrame {
        // TODO: disable estimation until jitter latency bug in str0m is fixed
        // if self.current.update(estimated_bitrate) {
        //     self.encoder
        //         .set_bit_rate((estimated_bitrate * 0.8).as_u64() as usize);
        //     self.encoder
        //         .set_max_bit_rate(estimated_bitrate.as_u64() as usize);
        // }

        // tracing::trace!("Encoding frame with bitrate: {}", self.current.bitrate);

        // let mut packet = ffmpeg_next::Packet::new(2 * 1024 * 1024);
        // let frame = make_frame(data, self.width, self.height);
        // self.encoder.set_time_base(duration.as_secs_f64());
        // self.encoder.send_frame(&frame).unwrap();
        // let _ = self.encoder.receive_packet(&mut packet);

        let pts = self.pts;
        let mut yuv = Vec::new();
        argb_to_i420(
            self.width as usize,
            self.height as usize,
            bgra_pixels,
            &mut yuv,
        );
        let packets = self.encoder.encode(pts, &yuv).unwrap();
        self.pts += duration.as_millis() as i64;

        let mut packet = Vec::new();

        for p in packets {
            packet.extend_from_slice(p.data);
        }

        self.stat.push(data.len() as u32);

        let current_bitrate = self.stat.bitrate();

        EncodedFrame {
            data: data.entirety().to_vec(),
            duration,
            current_bitrate,
            time,
        }
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

// fn make_frame(data: &[u8], width: u32, height: u32) -> ffmpeg_next::util::frame::video::Video {
//     unsafe {
//         let mut frame = ffmpeg_next::util::frame::video::Video::empty();
//         let avframe = frame.as_mut_ptr();
//         let pixel = Pixel::from(AV_PIX_FMT_0BGR32);
//         frame.alloc(pixel, width, height);
//         ffmpeg_next::sys::av_image_fill_arrays(
//             (*avframe).data.as_mut_ptr(),
//             (*avframe).linesize.as_mut_ptr(),
//             data.as_ptr(),
//             pixel.into(),
//             width as i32,
//             height as i32,
//             16,
//         );
//         frame
//     }
// }



fn argb_to_i420(width: usize, height: usize, src: &[u8], dest: &mut Vec<u8>) {
    let stride = src.len() / height;

    dest.clear();

    for y in 0..height {
        for x in 0..width {
            let o = y * stride + 4 * x;

            let b = src[o] as i32;
            let g = src[o + 1] as i32;
            let r = src[o + 2] as i32;

            let y = (66 * r + 129 * g + 25 * b + 128) / 256 + 16;
            dest.push(clamp(y));
        }
    }

    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let o = y * stride + 4 * x;

            let b = src[o] as i32;
            let g = src[o + 1] as i32;
            let r = src[o + 2] as i32;

            let u = (-38 * r - 74 * g + 112 * b + 128) / 256 + 128;
            dest.push(clamp(u));
        }
    }

    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let o = y * stride + 4 * x;

            let b = src[o] as i32;
            let g = src[o + 1] as i32;
            let r = src[o + 2] as i32;

            let v = (112 * r - 94 * g - 18 * b + 128) / 256 + 128;
            dest.push(clamp(v));
        }
    }
}

fn clamp(x: i32) -> u8 {
    x.min(255).max(0) as u8
}
