//! Prepared native processor path. No backend processing queues or meters on the callback.
use super::*;
use soniclaw_core::ProcessContext as CoreContext;
use std::{
    cell::{Cell, UnsafeCell},
    ptr,
};
use vst3::{
    Class, ComPtr, ComWrapper,
    Steinberg::{Vst::*, *},
};
const QUEUES: usize = 64;
struct ValueQueue {
    id: Cell<u32>,
    point: Cell<Option<(i32, f64)>>,
}
impl Class for ValueQueue {
    type Interfaces = (IParamValueQueue,);
}
impl IParamValueQueueTrait for ValueQueue {
    unsafe fn getParameterId(&self) -> u32 {
        self.id.get()
    }
    unsafe fn getPointCount(&self) -> i32 {
        i32::from(self.point.get().is_some())
    }
    unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut f64) -> i32 {
        if index != 0 || offset.is_null() || value.is_null() {
            return kResultFalse;
        }
        if let Some((o, v)) = self.point.get() {
            unsafe {
                *offset = o;
                *value = v;
            }
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn addPoint(&self, offset: i32, value: f64, index: *mut i32) -> i32 {
        if index.is_null() || !value.is_finite() || !(0.0..=1.0).contains(&value) || offset < 0 {
            return kResultFalse;
        }
        self.point.set(Some((offset, value)));
        unsafe {
            *index = 0;
        }
        kResultOk
    }
}
struct Changes {
    queues: Vec<ComWrapper<ValueQueue>>,
    used: Cell<usize>,
}
impl Changes {
    fn new() -> Self {
        Self {
            queues: (0..QUEUES)
                .map(|_| {
                    ComWrapper::new(ValueQueue {
                        id: Cell::new(0),
                        point: Cell::new(None),
                    })
                })
                .collect(),
            used: Cell::new(0),
        }
    }
    fn clear(&self) {
        for q in &self.queues[..self.used.get()] {
            q.point.set(None);
        }
        self.used.set(0);
    }
    fn push(&self, id: u32, value: f64) -> bool {
        let mut index = 0;
        let q = unsafe { self.addParameterData(&id, &mut index) };
        if q.is_null() {
            false
        } else {
            {
                self.queues[index as usize].point.set(Some((0, value)));
                true
            }
        }
    }
}
impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}
impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> i32 {
        self.used.get() as i32
    }
    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        if index < 0 || index as usize >= self.used.get() {
            return ptr::null_mut();
        }
        self.queues[index as usize]
            .as_com_ref::<IParamValueQueue>()
            .map_or(ptr::null_mut(), |p| p.as_ptr())
    }
    unsafe fn addParameterData(&self, id: *const u32, index: *mut i32) -> *mut IParamValueQueue {
        if id.is_null() || index.is_null() {
            return ptr::null_mut();
        }
        let id = unsafe { *id };
        let used = self.used.get();
        let slot = if let Some(i) = self.queues[..used].iter().position(|q| q.id.get() == id) {
            i
        } else {
            if used == QUEUES {
                return ptr::null_mut();
            }
            self.queues[used].id.set(id);
            self.queues[used].point.set(None);
            self.used.set(used + 1);
            used
        };
        unsafe {
            *index = slot as i32;
            self.getParameterData(slot as i32)
        }
    }
}
struct EmptyEvents;
impl Class for EmptyEvents {
    type Interfaces = (IEventList,);
}
impl IEventListTrait for EmptyEvents {
    unsafe fn getEventCount(&self) -> i32 {
        0
    }
    unsafe fn getEvent(&self, _: i32, _: *mut vst3::Steinberg::Vst::Event) -> i32 {
        kResultFalse
    }
    unsafe fn addEvent(&self, _: *mut vst3::Steinberg::Vst::Event) -> i32 {
        kResultFalse
    }
}
#[derive(Clone, Copy)]
pub(super) struct Command {
    pub id: u32,
    pub value: f64,
}
struct State {
    processor: ComPtr<IAudioProcessor>,
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    _in_ptrs: Box<[*mut f32; 2]>,
    _out_ptrs: Box<[*mut f32; 2]>,
    in_bus: Box<AudioBusBuffers>,
    out_bus: Box<AudioBusBuffers>,
    context: Box<ProcessContextNative>,
    changes: ComWrapper<Changes>,
    out_changes: ComWrapper<Changes>,
    empty: ComWrapper<EmptyEvents>,
    commands: rtrb::Consumer<Command>,
    has_input: bool,
}
type ProcessContextNative = vst3::Steinberg::Vst::ProcessContext;
impl State {
    #[allow(clippy::unnecessary_cast)] // VST3 sample-size constants differ in signedness across target ABIs.
    fn new(
        processor: ComPtr<IAudioProcessor>,
        spec: ProcessSpec,
        has_input: bool,
        commands: rtrb::Consumer<Command>,
    ) -> Result<Self> {
        if unsafe { processor.canProcessSampleSize(SymbolicSampleSizes_::kSample32 as i32) }
            != kResultOk
        {
            return Err("direct mode requires native f32 support".into());
        }
        let mut input = [
            vec![0.; spec.max_block_frames],
            vec![0.; spec.max_block_frames],
        ];
        let mut output = input.clone();
        let mut in_ptrs = Box::new([input[0].as_mut_ptr(), input[1].as_mut_ptr()]);
        let mut out_ptrs = Box::new([output[0].as_mut_ptr(), output[1].as_mut_ptr()]);
        let mut in_bus: Box<AudioBusBuffers> = Box::new(unsafe { std::mem::zeroed() });
        let mut out_bus = in_bus.clone();
        in_bus.numChannels = 2;
        out_bus.numChannels = 2;
        in_bus.__field0.channelBuffers32 = in_ptrs.as_mut_ptr();
        out_bus.__field0.channelBuffers32 = out_ptrs.as_mut_ptr();
        let mut context: Box<ProcessContextNative> = Box::new(unsafe { std::mem::zeroed() });
        context.sampleRate = spec.sample_rate;
        Ok(Self {
            processor,
            input,
            output,
            _in_ptrs: in_ptrs,
            _out_ptrs: out_ptrs,
            in_bus,
            out_bus,
            context,
            changes: ComWrapper::new(Changes::new()),
            out_changes: ComWrapper::new(Changes::new()),
            empty: ComWrapper::new(EmptyEvents),
            commands,
            has_input,
        })
    }
    #[allow(clippy::unnecessary_cast)] // VST3 flags differ in integer signedness across target ABIs.
    fn process(&mut self, b: &mut AudioBuffer<'_>, ctx: &CoreContext<'_>) -> bool {
        let n = b.frames();
        if n == 0 {
            return true;
        }
        self.changes.clear();
        self.out_changes.clear();
        for _ in 0..QUEUES {
            let Ok(c) = self.commands.pop() else {
                break;
            };
            if !self.changes.push(c.id, c.value) {
                return false;
            }
        }
        for (i, f) in b.samples().as_chunks::<2>().0.iter().enumerate() {
            self.input[0][i] = f[0];
            self.input[1][i] = f[1];
        }
        self.output[0][..n].fill(0.);
        self.output[1][..n].fill(0.);
        self.in_bus.silenceFlags = 0;
        self.out_bus.silenceFlags = 0;
        self.context.state = (ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | if ctx.transport.playing {
                ProcessContext_::StatesAndFlags_::kPlaying
            } else {
                0
            }) as u32;
        self.context.tempo = ctx.transport.tempo;
        self.context.projectTimeSamples = ctx.transport.sample_position.min(i64::MAX as u64) as i64;
        self.context.projectTimeMusic =
            ctx.transport.sample_position as f64 / self.context.sampleRate * ctx.transport.tempo
                / 60.;
        let mut data: ProcessData = unsafe { std::mem::zeroed() };
        data.processMode = ProcessModes_::kRealtime as i32;
        data.symbolicSampleSize = SymbolicSampleSizes_::kSample32 as i32;
        data.numSamples = n as i32;
        data.numInputs = i32::from(self.has_input);
        data.numOutputs = 1;
        data.inputs = if self.has_input {
            &mut *self.in_bus
        } else {
            ptr::null_mut()
        };
        data.outputs = &mut *self.out_bus;
        data.processContext = &mut *self.context;
        data.inputParameterChanges = self
            .changes
            .as_com_ref::<IParameterChanges>()
            .expect("interface")
            .as_ptr();
        data.outputParameterChanges = self
            .out_changes
            .as_com_ref::<IParameterChanges>()
            .expect("interface")
            .as_ptr();
        data.inputEvents = self
            .empty
            .as_com_ref::<IEventList>()
            .expect("interface")
            .as_ptr();
        data.outputEvents = data.inputEvents;
        if unsafe { self.processor.process(&mut data) } != kResultOk {
            return false;
        }
        for (i, f) in b
            .samples_mut()
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .enumerate()
        {
            f[0] = self.output[0][i];
            f[1] = self.output[1][i];
        }
        true
    }
}
pub(super) struct Shared {
    busy: AtomicBool,
    stop: AtomicBool,
    state: UnsafeCell<Option<State>>,
    pub errors: AtomicU64,
}
// SAFETY: only the successful busy-CAS holder accesses native state. Owner revokes
// after setting stop and waits for an in-flight callback before taking/dropping state.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}
struct Guard<'a>(&'a AtomicBool);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Shared {
    pub(super) fn revoke(&self) {
        self.stop.store(true, Ordering::Release);
        while self
            .busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::thread::yield_now();
        }
        let _g = Guard(&self.busy);
        unsafe {
            (*self.state.get()).take();
        }
    }
}
pub struct DirectProcessor {
    shared: Arc<Shared>,
    spec: ProcessSpec,
    latency: usize,
}
impl AudioProcessor for DirectProcessor {
    fn prepare(&mut self, s: ProcessSpec) -> std::result::Result<(), Error> {
        s.validate()?;
        if s.channels != 2
            || s.sample_rate != self.spec.sample_rate
            || s.max_block_frames > self.spec.max_block_frames
        {
            return Err(Error::InvalidSpec);
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
    fn latency_frames(&self) -> usize {
        self.latency
    }
    fn process(&mut self, b: &mut AudioBuffer<'_>, ctx: &CoreContext<'_>) {
        if self
            .shared
            .busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            b.clear();
            return;
        }
        let _g = Guard(&self.shared.busy);
        if self.shared.stop.load(Ordering::Acquire)
            || b.channels() != 2
            || b.frames() > self.spec.max_block_frames
            || !ctx.events.is_empty()
            || !ctx.transport.tempo.is_finite()
            || !(1.0..=1000.0).contains(&ctx.transport.tempo)
        {
            b.clear();
            self.shared.errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let state = unsafe { &mut *self.shared.state.get() };
        if !state.as_mut().is_some_and(|s| s.process(b, ctx)) {
            b.clear();
            self.shared.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}
impl Drop for DirectProcessor {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}
impl Instance {
    /// Prepared direct mode: audio/normalized controls; MIDI/editor integration is not active in this first direct path.
    pub fn start_direct(&mut self) -> Result<DirectProcessor> {
        self.stop()?;
        self.close_editor();
        let (processor, latency, has_input) = {
            let mut p = self.lock()?;
            p.service_run_loop();
            p.service_host_requests().map_err(err)?;
            p.start_processing().map_err(err)?;
            p.service_host_requests().map_err(err)?;
            let processor = unsafe { p.direct_audio_processor() }
                .ok_or("backend cannot provide native direct processor")?;
            (
                processor,
                p.latency_samples() as usize,
                !p.audio_bus_layout().map_err(err)?.inputs.is_empty(),
            )
        };
        let (tx, rx) = rtrb::RingBuffer::new(QUEUES);
        let state = State::new(processor, self.spec, has_input, rx)?;
        let shared = Arc::new(Shared {
            busy: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            state: UnsafeCell::new(Some(state)),
            errors: AtomicU64::new(0),
        });
        self.direct = Some(shared.clone());
        self.direct_commands = Some(tx);
        Ok(DirectProcessor {
            shared,
            spec: self.spec,
            latency,
        })
    }
    pub fn direct_errors(&self) -> u64 {
        self.direct
            .as_ref()
            .map_or(0, |s| s.errors.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parameter_pool_is_bounded_and_reuses_slots() {
        let changes = Changes::new();
        for id in 0..QUEUES as u32 {
            assert!(changes.push(id, 0.5));
        }
        assert!(!changes.push(1000, 0.5));
        assert!(changes.push(0, 0.75));
        assert_eq!(changes.queues[0].point.get(), Some((0, 0.75)));
        changes.clear();
        assert_eq!(changes.used.get(), 0);
        assert!(changes.push(999, 0.25));
        assert_eq!(changes.queues[0].id.get(), 999);
    }
    #[test]
    fn revoked_callback_remains_silent_without_allocation() {
        let shared = Arc::new(Shared {
            busy: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            state: UnsafeCell::new(None),
            errors: AtomicU64::new(0),
        });
        let mut p = DirectProcessor {
            shared: shared.clone(),
            spec: ProcessSpec {
                sample_rate: 48000.,
                max_block_frames: 256,
                channels: 2,
            },
            latency: 0,
        };
        shared.revoke();
        let mut data = [1.; 512];
        let ctx = CoreContext {
            events: &[],
            transport: Default::default(),
        };
        let count = allocation_counter::measure(|| {
            p.process(&mut AudioBuffer::new(&mut data, 2).unwrap(), &ctx)
        });
        assert_eq!(count.count_total, 0);
        assert_eq!(data, [0.; 512]);
    }
}
