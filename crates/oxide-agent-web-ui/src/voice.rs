use crate::auth::AuthContext;
use crate::utils::spawn_ui;
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;

const WAVEFORM_BARS: usize = 32;
const MEDIA_RECORDER_TIMESLICE_MS: i32 = 250;

static NEXT_RECORDING_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ACTIVE_RECORDINGS: RefCell<HashMap<u64, VoiceRecordingSlot>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceRecorderStatus {
    Idle,
    RequestingPermission,
    Recording,
    Finalizing,
    Transcribing,
}

struct ActiveVoiceRecording {
    recorder: web_sys::MediaRecorder,
    stream: web_sys::MediaStream,
    audio_context: Option<web_sys::AudioContext>,
    _source: Option<web_sys::MediaStreamAudioSourceNode>,
    interval: Option<Interval>,
    data_callback: Option<Closure<dyn FnMut(web_sys::BlobEvent)>>,
    stop_callback: Option<Closure<dyn FnMut()>>,
    error_callback: Option<Closure<dyn FnMut(web_sys::Event)>>,
}

impl ActiveVoiceRecording {
    fn stop_capture(&mut self) {
        self.interval.take();
        stop_media_stream_tracks(&self.stream);
        if let Some(context) = self.audio_context.take() {
            let _ = context.close();
        }
    }

    fn detach_callbacks(&mut self) {
        self.recorder.set_ondataavailable(None);
        self.recorder.set_onstop(None);
        self.recorder.set_onerror(None);
        self.data_callback.take();
        self.stop_callback.take();
        self.error_callback.take();
    }

    fn cancel(mut self) {
        self.detach_callbacks();
        self.stop_capture();
        let _ = self.recorder.stop();
    }
}

enum VoiceRecordingSlot {
    Pending,
    Active(ActiveVoiceRecording),
}

#[component]
pub(crate) fn VoiceRecorderControl(
    auth: AuthContext,
    disabled: Signal<bool>,
    set_busy: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_transcript: Callback<String>,
) -> impl IntoView {
    let recording_id = NEXT_RECORDING_ID.fetch_add(1, Ordering::Relaxed);
    let (status, set_status) = signal(VoiceRecorderStatus::Idle);
    let (elapsed_ms, set_elapsed_ms) = signal(0_u32);
    let (waveform, set_waveform) = signal(idle_waveform());

    on_cleanup(move || {
        cancel_recording_by_id(recording_id);
        set_busy.set(false);
    });

    let start = Callback::new(move |_| {
        if disabled.get_untracked() || status.get_untracked() != VoiceRecorderStatus::Idle {
            return;
        }
        if !begin_recording_slot(recording_id) {
            return;
        }
        set_voice_status(
            VoiceRecorderStatus::RequestingPermission,
            set_status,
            set_busy,
        );
        set_error.set(None);
        set_elapsed_ms.set(0);
        set_waveform.set(idle_waveform());

        spawn_ui(async move {
            match start_recording(
                recording_id,
                auth,
                set_status,
                set_busy,
                set_error,
                set_elapsed_ms,
                set_waveform,
                on_transcript,
            )
            .await
            {
                Ok(recording) => match activate_recording_slot(recording_id, recording) {
                    Ok(()) => {
                        set_voice_status(VoiceRecorderStatus::Recording, set_status, set_busy);
                    }
                    Err(recording) => recording.cancel(),
                },
                Err(error) => {
                    if clear_pending_recording_slot(recording_id) {
                        set_error.set(Some(error));
                        set_wave_status_idle(set_status, set_busy, set_elapsed_ms, set_waveform);
                    }
                }
            }
        });
    });

    let confirm = Callback::new(move |_| {
        finish_recording(recording_id, set_status, set_busy, set_error);
    });

    let cancel = Callback::new(move |_| {
        cancel_active_recording(
            recording_id,
            set_status,
            set_busy,
            set_elapsed_ms,
            set_waveform,
        );
    });

    view! {
        <div class="voice-recorder" class:voice-active=move || status.get() != VoiceRecorderStatus::Idle>
            {move || match status.get() {
                VoiceRecorderStatus::Idle => view! {
                    <button
                        class="voice-button"
                        type="button"
                        title="Record voice message"
                        aria-label="Record voice message"
                        disabled=move || disabled.get()
                        on:click=move |ev| start.run(ev)
                    >
                        "🎙"
                    </button>
                }.into_any(),
                VoiceRecorderStatus::RequestingPermission => view! {
                    <div class="voice-pill" role="status" aria-live="polite">
                        <span class="voice-status-copy">"Requesting microphone…"</span>
                    </div>
                }.into_any(),
                VoiceRecorderStatus::Recording => view! {
                    <div class="voice-pill recording" role="group" aria-label="Voice recording controls">
                        <button class="voice-action" type="button" title="Cancel recording" aria-label="Cancel recording" on:click=move |ev| cancel.run(ev)>"×"</button>
                        <span class="voice-timer">{move || format_elapsed(elapsed_ms.get())}</span>
                        <WaveformBars levels=waveform />
                        <button class="voice-action confirm" type="button" title="Stop and transcribe" aria-label="Stop and transcribe" on:click=move |ev| confirm.run(ev)>"✓"</button>
                    </div>
                }.into_any(),
                VoiceRecorderStatus::Finalizing => view! {
                    <div class="voice-pill" role="status" aria-live="polite">
                        <span class="voice-timer">{format_elapsed(elapsed_ms.get_untracked())}</span>
                        <WaveformBars levels=waveform />
                        <span class="voice-status-copy">"Finalizing…"</span>
                    </div>
                }.into_any(),
                VoiceRecorderStatus::Transcribing => view! {
                    <div class="voice-pill" role="status" aria-live="polite">
                        <span class="voice-timer">{format_elapsed(elapsed_ms.get_untracked())}</span>
                        <WaveformBars levels=waveform />
                        <span class="voice-status-copy">"Transcribing…"</span>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn WaveformBars(levels: ReadSignal<Vec<u8>>) -> impl IntoView {
    view! {
        <span class="voice-waveform" aria-hidden="true">
            {move || levels.get().into_iter().enumerate().map(|(index, level)| {
                let height = 4_u8.saturating_add(level.min(28));
                view! {
                    <span
                        class="voice-waveform-bar"
                        style=format!("height:{height}px; animation-delay:{}ms", (index % 8) * 35)
                    />
                }
            }).collect_view()}
        </span>
    }
}

async fn start_recording(
    recording_id: u64,
    auth: AuthContext,
    set_status: WriteSignal<VoiceRecorderStatus>,
    set_busy: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    set_elapsed_ms: WriteSignal<u32>,
    set_waveform: WriteSignal<Vec<u8>>,
    on_transcript: Callback<String>,
) -> Result<ActiveVoiceRecording, String> {
    let window = web_sys::window().ok_or_else(|| "Browser window is unavailable.".to_string())?;
    let media_devices = window
        .navigator()
        .media_devices()
        .map_err(|_| "Microphone recording requires HTTPS or localhost.".to_string())?;
    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio_bool(true);
    let stream = JsFuture::from(
        media_devices
            .get_user_media_with_constraints(&constraints)
            .map_err(js_error_message)?,
    )
    .await
    .map_err(js_error_message)?
    .dyn_into::<web_sys::MediaStream>()
    .map_err(|_| "Browser returned an invalid microphone stream.".to_string())?;

    let recorder = media_recorder_for_stream(&stream).map_err(|error| {
        stop_media_stream_tracks(&stream);
        error
    })?;
    let recorder_mime_type = recorder.mime_type();
    let mime_type = if recorder_mime_type.trim().is_empty() {
        preferred_audio_mime_type().unwrap_or_else(|| "audio/webm".to_string())
    } else {
        recorder_mime_type
    };
    let chunks = std::rc::Rc::new(RefCell::new(Vec::<web_sys::Blob>::new()));
    let started_at = js_sys::Date::now();
    let visualizer = build_visualizer(&stream, started_at, set_elapsed_ms, set_waveform);

    let data_chunks = std::rc::Rc::clone(&chunks);
    let data_callback = Closure::wrap(Box::new(move |event: web_sys::BlobEvent| {
        if let Some(data) = event.data()
            && data.size() > 0.0
        {
            data_chunks.borrow_mut().push(data);
        }
    }) as Box<dyn FnMut(_)>);
    recorder.set_ondataavailable(Some(data_callback.as_ref().unchecked_ref()));

    let stop_chunks = std::rc::Rc::clone(&chunks);
    let stop_mime_type = mime_type.clone();
    let stop_callback = Closure::wrap(Box::new(move || {
        let duration_ms = elapsed_duration_ms(started_at);
        let chunks = stop_chunks.borrow().clone();
        if let Some(mut recording) = take_active_recording(recording_id) {
            recording.detach_callbacks();
            recording.stop_capture();
        }
        if chunks.is_empty() {
            set_error.set(Some("Voice recording produced no audio data.".to_string()));
            set_wave_status_idle(set_status, set_busy, set_elapsed_ms, set_waveform);
            return;
        }
        set_voice_status(VoiceRecorderStatus::Transcribing, set_status, set_busy);
        let file = match voice_file_from_chunks(&chunks, &stop_mime_type, duration_ms) {
            Ok(file) => file,
            Err(error) => {
                set_error.set(Some(error));
                set_wave_status_idle(set_status, set_busy, set_elapsed_ms, set_waveform);
                return;
            }
        };

        spawn_ui(async move {
            match auth
                .client()
                .transcribe_voice(&file, Some(duration_ms))
                .await
            {
                Ok(response) => {
                    let transcript = response.text.trim().to_string();
                    if transcript.is_empty() {
                        set_error.set(Some("Voice transcription returned empty text.".to_string()));
                    } else {
                        on_transcript.run(transcript);
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_wave_status_idle(set_status, set_busy, set_elapsed_ms, set_waveform);
        });
    }) as Box<dyn FnMut()>);
    recorder.set_onstop(Some(stop_callback.as_ref().unchecked_ref()));

    let error_callback = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        cancel_recording_by_id(recording_id);
        set_error.set(Some("Voice recording failed in the browser.".to_string()));
        set_wave_status_idle(set_status, set_busy, set_elapsed_ms, set_waveform);
    }) as Box<dyn FnMut(_)>);
    recorder.set_onerror(Some(error_callback.as_ref().unchecked_ref()));

    if let Err(error) = recorder.start_with_time_slice(MEDIA_RECORDER_TIMESLICE_MS) {
        stop_media_stream_tracks(&stream);
        if let Some(context) = &visualizer.audio_context {
            let _ = context.close();
        }
        return Err(js_error_message(error));
    }

    Ok(ActiveVoiceRecording {
        recorder,
        stream,
        audio_context: visualizer.audio_context,
        _source: visualizer.source,
        interval: Some(visualizer.interval),
        data_callback: Some(data_callback),
        stop_callback: Some(stop_callback),
        error_callback: Some(error_callback),
    })
}

struct VoiceVisualizer {
    audio_context: Option<web_sys::AudioContext>,
    source: Option<web_sys::MediaStreamAudioSourceNode>,
    interval: Interval,
}

fn build_visualizer(
    stream: &web_sys::MediaStream,
    started_at: f64,
    set_elapsed_ms: WriteSignal<u32>,
    set_waveform: WriteSignal<Vec<u8>>,
) -> VoiceVisualizer {
    let audio_context = web_sys::AudioContext::new().ok();
    let analyser = audio_context
        .as_ref()
        .and_then(|context| context.create_analyser().ok());
    let source = audio_context
        .as_ref()
        .and_then(|context| context.create_media_stream_source(stream).ok());
    if let (Some(source), Some(analyser)) = (&source, &analyser) {
        analyser.set_fft_size(64);
        let _ = source.connect_with_audio_node(analyser.unchecked_ref::<web_sys::AudioNode>());
    }

    let interval_analyser = analyser.clone();
    let interval = Interval::new(100, move || {
        let elapsed = elapsed_duration_ms(started_at);
        set_elapsed_ms.set(elapsed);
        if let Some(analyser) = &interval_analyser {
            let mut samples = vec![128_u8; analyser.frequency_bin_count() as usize];
            analyser.get_byte_time_domain_data(&mut samples);
            set_waveform.set(waveform_from_time_domain(&samples));
        } else {
            set_waveform.set(animated_waveform(elapsed));
        }
    });

    VoiceVisualizer {
        audio_context,
        source,
        interval,
    }
}

fn finish_recording(
    recording_id: u64,
    set_status: WriteSignal<VoiceRecorderStatus>,
    set_busy: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
) {
    set_voice_status(VoiceRecorderStatus::Finalizing, set_status, set_busy);
    let stop_result = ACTIVE_RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        let Some(VoiceRecordingSlot::Active(recording)) = recordings.get_mut(&recording_id) else {
            return None;
        };
        recording.stop_capture();
        Some(recording.recorder.stop())
    });

    match stop_result {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            cancel_recording_by_id(recording_id);
            set_error.set(Some(format!(
                "Failed to stop voice recording: {}",
                js_error_message(error)
            )));
            set_voice_status(VoiceRecorderStatus::Idle, set_status, set_busy);
        }
        None => set_voice_status(VoiceRecorderStatus::Idle, set_status, set_busy),
    }
}

fn cancel_active_recording(
    recording_id: u64,
    set_status: WriteSignal<VoiceRecorderStatus>,
    set_busy: WriteSignal<bool>,
    set_elapsed_ms: WriteSignal<u32>,
    set_waveform: WriteSignal<Vec<u8>>,
) {
    cancel_recording_by_id(recording_id);
    set_wave_status_idle(set_status, set_busy, set_elapsed_ms, set_waveform);
}

fn begin_recording_slot(recording_id: u64) -> bool {
    ACTIVE_RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        if recordings.contains_key(&recording_id) {
            false
        } else {
            recordings.insert(recording_id, VoiceRecordingSlot::Pending);
            true
        }
    })
}

fn activate_recording_slot(
    recording_id: u64,
    recording: ActiveVoiceRecording,
) -> Result<(), ActiveVoiceRecording> {
    ACTIVE_RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        match recordings.get_mut(&recording_id) {
            Some(slot @ VoiceRecordingSlot::Pending) => {
                *slot = VoiceRecordingSlot::Active(recording);
                Ok(())
            }
            _ => Err(recording),
        }
    })
}

fn clear_pending_recording_slot(recording_id: u64) -> bool {
    ACTIVE_RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        if matches!(
            recordings.get(&recording_id),
            Some(VoiceRecordingSlot::Pending)
        ) {
            recordings.remove(&recording_id);
            true
        } else {
            false
        }
    })
}

fn take_active_recording(recording_id: u64) -> Option<ActiveVoiceRecording> {
    ACTIVE_RECORDINGS.with(
        |recordings| match recordings.borrow_mut().remove(&recording_id) {
            Some(VoiceRecordingSlot::Active(recording)) => Some(recording),
            _ => None,
        },
    )
}

fn cancel_recording_by_id(recording_id: u64) {
    if let Some(recording) = take_active_recording(recording_id) {
        recording.cancel();
    }
}

fn media_recorder_for_stream(
    stream: &web_sys::MediaStream,
) -> Result<web_sys::MediaRecorder, String> {
    if let Some(mime_type) = preferred_audio_mime_type() {
        let options = web_sys::MediaRecorderOptions::new();
        options.set_mime_type(&mime_type);
        web_sys::MediaRecorder::new_with_media_stream_and_media_recorder_options(stream, &options)
            .map_err(js_error_message)
    } else {
        web_sys::MediaRecorder::new_with_media_stream(stream).map_err(js_error_message)
    }
}

fn preferred_audio_mime_type() -> Option<String> {
    [
        "audio/webm;codecs=opus",
        "audio/ogg;codecs=opus",
        "audio/mp4",
        "audio/webm",
    ]
    .iter()
    .find(|mime_type| web_sys::MediaRecorder::is_type_supported(mime_type))
    .map(|mime_type| (*mime_type).to_string())
}

fn voice_file_from_chunks(
    chunks: &[web_sys::Blob],
    mime_type: &str,
    duration_ms: u32,
) -> Result<web_sys::File, String> {
    let parts = js_sys::Array::new();
    for chunk in chunks {
        parts.push(chunk.as_ref());
    }
    let options = web_sys::FilePropertyBag::new();
    options.set_type(mime_type);
    let file_name = format!(
        "voice-{}.{}",
        js_sys::Date::now() as u64,
        extension_from_mime_type(mime_type)
    );
    let file =
        web_sys::File::new_with_blob_sequence_and_options(parts.as_ref(), &file_name, &options)
            .map_err(js_error_message)?;
    if file.size() <= 0.0 {
        return Err("Voice recording produced an empty file.".to_string());
    }
    if duration_ms == 0 {
        return Err("Voice recording was too short to transcribe.".to_string());
    }
    Ok(file)
}

fn extension_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type.split(';').next().unwrap_or(mime_type).trim() {
        "audio/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/flac" => "flac",
        _ => "webm",
    }
}

fn stop_media_stream_tracks(stream: &web_sys::MediaStream) {
    let tracks = stream.get_tracks();
    for index in 0..tracks.length() {
        if let Ok(track) = tracks.get(index).dyn_into::<web_sys::MediaStreamTrack>() {
            track.stop();
        }
    }
}

fn waveform_from_time_domain(samples: &[u8]) -> Vec<u8> {
    if samples.is_empty() {
        return idle_waveform();
    }
    let chunk_size = samples.len().div_ceil(WAVEFORM_BARS).max(1);
    (0..WAVEFORM_BARS)
        .map(|bar| {
            let start = bar * chunk_size;
            let end = ((bar + 1) * chunk_size).min(samples.len());
            let slice = &samples[start.min(samples.len())..end.min(samples.len())];
            if slice.is_empty() {
                return 2;
            }
            let total: u32 = slice
                .iter()
                .map(|sample| i16::from(*sample).saturating_sub(128).unsigned_abs() as u32)
                .sum();
            ((total / slice.len() as u32) / 2).clamp(2, 28) as u8
        })
        .collect()
}

fn animated_waveform(elapsed_ms: u32) -> Vec<u8> {
    let phase = (elapsed_ms / 100) as usize;
    (0..WAVEFORM_BARS)
        .map(|index| {
            let value = ((index + phase) % 9) as u8;
            4 + value.saturating_mul(2)
        })
        .collect()
}

fn idle_waveform() -> Vec<u8> {
    vec![4; WAVEFORM_BARS]
}

fn set_wave_status_idle(
    set_status: WriteSignal<VoiceRecorderStatus>,
    set_busy: WriteSignal<bool>,
    set_elapsed_ms: WriteSignal<u32>,
    set_waveform: WriteSignal<Vec<u8>>,
) {
    set_elapsed_ms.set(0);
    set_waveform.set(idle_waveform());
    set_voice_status(VoiceRecorderStatus::Idle, set_status, set_busy);
}

fn set_voice_status(
    status: VoiceRecorderStatus,
    set_status: WriteSignal<VoiceRecorderStatus>,
    set_busy: WriteSignal<bool>,
) {
    set_status.set(status);
    set_busy.set(status != VoiceRecorderStatus::Idle);
}

fn elapsed_duration_ms(started_at: f64) -> u32 {
    (js_sys::Date::now() - started_at)
        .max(0.0)
        .min(u32::MAX as f64) as u32
}

fn format_elapsed(milliseconds: u32) -> String {
    let seconds = milliseconds / 1_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn js_error_message(error: JsValue) -> String {
    if let Some(message) = error.as_string() {
        return message;
    }
    js_sys::JSON::stringify(&error)
        .ok()
        .and_then(|value| value.as_string())
        .filter(|value| !value.is_empty() && value != "{}")
        .unwrap_or_else(|| format!("{error:?}"))
}
