//! Latest-wins render scheduling (plan §6.10 #3): parameter updates
//! coalesce while a slider is dragged. At most one render is in flight and
//! at most one is pending; a newer request replaces a pending older one, so
//! stale frames are never rendered.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::pipeline::{Histogram, Pipeline, RenderRequest, RenderStats};

/// A finished frame. `texture` is the pipeline's `Rgba8Unorm` output
/// (shared, not copied: the app samples it directly).
#[derive(Debug, Clone)]
pub struct RenderResult {
    /// The id `submit` returned for the request that produced this frame.
    pub id: u64,
    pub stats: Option<RenderStats>,
    pub texture: Option<wgpu::Texture>,
    pub histogram: Option<Histogram>,
    pub error: Option<String>,
}

#[derive(Default)]
struct State {
    pending: Option<(u64, RenderRequest)>,
    result: Option<RenderResult>,
    next_id: u64,
    completed: u64,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct RenderLoop {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RenderLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderLoop").finish_non_exhaustive()
    }
}

impl RenderLoop {
    /// Moves `pipeline` onto a worker thread.
    pub fn spawn(mut pipeline: Pipeline) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("viberoom-render".into())
            .spawn(move || {
                loop {
                    let (id, req) = {
                        let Ok(mut st) = worker_shared.state.lock() else {
                            return;
                        };
                        loop {
                            if st.stop {
                                return;
                            }
                            if let Some(job) = st.pending.take() {
                                break job;
                            }
                            st = match worker_shared.wake.wait(st) {
                                Ok(g) => g,
                                Err(_) => return,
                            };
                        }
                    };
                    let result = match pipeline.render(&req) {
                        Ok(stats) => RenderResult {
                            id,
                            stats: Some(stats),
                            texture: pipeline.output_texture().cloned(),
                            histogram: pipeline.histogram().cloned(),
                            error: None,
                        },
                        Err(e) => RenderResult {
                            id,
                            stats: None,
                            texture: None,
                            histogram: None,
                            error: Some(e.to_string()),
                        },
                    };
                    if let Ok(mut st) = worker_shared.state.lock() {
                        st.result = Some(result);
                        st.completed += 1;
                    }
                }
            })
            .ok();
        Self { shared, worker }
    }

    /// Queues `req`, replacing any request still waiting. Returns its id.
    pub fn submit(&self, req: RenderRequest) -> u64 {
        let Ok(mut st) = self.shared.state.lock() else {
            return 0;
        };
        st.next_id += 1;
        let id = st.next_id;
        st.pending = Some((id, req));
        self.shared.wake.notify_one();
        id
    }

    /// The newest finished frame not yet taken.
    pub fn take_result(&self) -> Option<RenderResult> {
        self.shared.state.lock().ok()?.result.take()
    }

    /// How many renders have actually run (submits minus dropped ones).
    pub fn completed(&self) -> u64 {
        self.shared.state.lock().map(|s| s.completed).unwrap_or(0)
    }
}

impl Drop for RenderLoop {
    fn drop(&mut self) {
        if let Ok(mut st) = self.shared.state.lock() {
            st.stop = true;
        }
        self.shared.wake.notify_all();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gpu::GpuContext;
    use crate::ops::{Exposure, ExposureParams};
    use crate::params::EditParams;
    use viberoom_color::cie::SRGB;
    use viberoom_io::{CameraColor, DecodedImage, ImageF32};
    use std::time::{Duration, Instant};

    #[test]
    fn a_burst_of_updates_renders_far_fewer_frames_and_ends_on_the_last() {
        let Some(g) = GpuContext::headless() else {
            return;
        };
        let camera = CameraColor {
            xyz_to_camera: SRGB.xyz_to_rgb().to_f32(),
            d65_mul: [1.0; 3],
            as_shot_mul: None,
        };
        let rgb = ImageF32 {
            width: 512,
            height: 512,
            channels: 3,
            data: vec![0.2; 512 * 512 * 3],
        };
        let pipeline = Pipeline::new(&g, &DecodedImage::SceneLinear { rgb, camera }).unwrap();
        let rl = RenderLoop::spawn(pipeline);

        let mut last = 0;
        for i in 0..200 {
            let mut p = EditParams::default();
            p.set::<Exposure>(ExposureParams {
                ev: i as f64 * 0.01,
            });
            last = rl.submit(RenderRequest::new(p, 512));
        }
        let start = Instant::now();
        let res = loop {
            if let Some(r) = rl.take_result()
                && r.id == last
            {
                break r;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(res.error.is_none(), "{:?}", res.error);
        assert!(
            rl.completed() < 200,
            "stale frames must be dropped, ran {}",
            rl.completed()
        );
    }
}
