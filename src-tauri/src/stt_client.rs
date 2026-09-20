//! Cloud speech-to-text over an OpenAI-compatible `/audio/transcriptions`
//! endpoint.
//!
//! Handy transcribes locally by default. When cloud STT is enabled the PCM that
//! would have gone to the local engine is encoded as a WAV and posted to a
//! remote endpoint instead. The request shape is the OpenAI one (multipart with
//! a `file` part), so the same setting works against OpenAI, OpenRouter, Groq or
//! a self-hosted gateway — only the base URL and the model id change.
//!
//! Provider credentials and base URLs are shared with post-processing: the
//! provider is looked up in `post_process_providers` by id and the key comes
//! from `post_process_api_keys`, so a key entered once serves both features.

use std::io::Cursor;
use std::time::Duration;

use log::debug;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::settings::AppSettings;

/// Cloud transcription is a network round trip on the critical path between the
/// user releasing the shortcut and text appearing, so the ceiling is generous
/// enough for a long dictation but still bounded.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Deserialize)]
struct TranscriptionResponse {
    /// Every OpenAI-compatible transcription endpoint returns the text here,
    /// both for `response_format: json` and `verbose_json`.
    text: Option<String>,
}

/// Encode mono f32 PCM as a 16-bit WAV in memory.
///
/// Handy's capture pipeline is fixed at [`WHISPER_SAMPLE_RATE`] mono, and every
/// transcription endpoint accepts 16 kHz WAV, so no resampling is needed.
fn encode_wav(audio: &[f32]) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut buffer, spec)
            .map_err(|e| format!("Failed to start WAV encoding: {}", e))?;
        for sample in audio {
            // Clamp before scaling: the capture path can overshoot [-1.0, 1.0]
            // slightly and `as i16` would wrap a clipped peak into the opposite
            // polarity, which is audible as a click to the model.
            let clamped = sample.clamp(-1.0, 1.0);
            let scaled = (clamped * i16::MAX as f32) as i16;
            writer
                .write_sample(scaled)
                .map_err(|e| format!("Failed to encode audio sample: {}", e))?;
        }
        writer
            .finalize()
            .map_err(|e| format!("Failed to finalize WAV encoding: {}", e))?;
    }

    Ok(buffer.into_inner())
}

/// Build the vocabulary hint sent with the request.
///
/// Transcription endpoints use it to bias spelling, which is exactly what the
/// custom-words list does locally — so that one list serves both paths and
/// there is no second place to keep in sync.
fn build_prompt(settings: &AppSettings) -> String {
    settings
        .custom_words
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Join a provider base URL with the transcriptions path.
fn transcriptions_url(base_url: &str) -> String {
    format!("{}/audio/transcriptions", base_url.trim_end_matches('/'))
}

/// Transcribe `audio` through the configured cloud provider.
///
/// `language` is the caller's resolved language intent; `None` (or `"auto"`)
/// lets the provider detect it, which is what mixed-language dictation wants.
pub fn transcribe_with_cloud(
    settings: &AppSettings,
    audio: &[f32],
    language: Option<&str>,
) -> Result<String, String> {
    let provider_id = settings.cloud_stt_provider_id.trim();
    if provider_id.is_empty() {
        return Err("Cloud transcription is enabled but no provider is selected".to_string());
    }

    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("Unknown cloud transcription provider '{}'", provider_id))?;

    if !provider.base_url.starts_with("http") {
        return Err(format!(
            "Provider '{}' has no HTTP endpoint and cannot be used for transcription",
            provider.id
        ));
    }

    let model = settings.cloud_stt_model.trim();
    if model.is_empty() {
        return Err("Cloud transcription is enabled but no model is set".to_string());
    }

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if api_key.trim().is_empty() {
        return Err(format!("No API key configured for '{}'", provider.label));
    }

    let wav = encode_wav(audio)?;
    let url = transcriptions_url(&provider.base_url);

    debug!(
        "Cloud transcription: provider '{}', model '{}', {} bytes of WAV",
        provider.id,
        model,
        wav.len()
    );

    let mut headers = HeaderMap::new();
    let mut auth = HeaderValue::from_str(&format!("Bearer {}", api_key.trim()))
        .map_err(|_| "API key contains characters that cannot be sent in a header".to_string())?;
    auth.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth);

    let mut form = Form::new().text("model", model.to_string()).part(
        "file",
        Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| format!("Failed to build request: {}", e))?,
    );

    // "auto" is Handy's internal sentinel for "let the model decide"; the
    // OpenAI shape expresses that by omitting the field entirely.
    if let Some(language) = language.filter(|l| !l.is_empty() && *l != "auto") {
        form = form.text("language", language.to_string());
    }

    // The words the user listed under custom words, handed to the model as a
    // vocabulary hint so product names and technical terms survive.
    let prompt = build_prompt(settings);
    if !prompt.is_empty() {
        form = form.text("prompt", prompt);
    }

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .default_headers(headers)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // `transcribe` is synchronous, but one of its callers reaches it from
    // inside an async task (`actions.rs` transcribes within `spawn`), so the
    // calling thread can be a Tokio worker. Blocking on a request there would
    // start a runtime inside a runtime and hang forever, so the request gets
    // its own OS thread and this one simply waits for it — the same shape as
    // the blocking local inference that happens at this point otherwise.
    let request = move || {
        tauri::async_runtime::block_on(async move {
            let response = client
                .post(&url)
                .multipart(form)
                .send()
                .await
                .map_err(|e| format!("Transcription request failed: {}", e))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| format!("Failed to read transcription response: {}", e))?;

            if !status.is_success() {
                // Provider error bodies are short and actionable (missing credit,
                // unknown model, age gate), so surfacing a trimmed copy saves a
                // round trip through the logs.
                let detail = body.chars().take(300).collect::<String>();
                return Err(format!("Transcription failed ({}): {}", status, detail));
            }

            let parsed: TranscriptionResponse = serde_json::from_str(&body)
                .map_err(|e| format!("Could not parse transcription response: {}", e))?;

            Ok(parsed.text.unwrap_or_default().trim().to_string())
        })
    };

    std::thread::spawn(request)
        .join()
        .map_err(|_| "Transcription request thread panicked".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_wav_with_a_riff_header() {
        let wav = encode_wav(&[0.0, 0.5, -0.5]).expect("encoding should succeed");
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }

    #[test]
    fn clamps_out_of_range_samples_instead_of_wrapping() {
        let wav = encode_wav(&[2.0, -2.0]).expect("encoding should succeed");
        let mut reader = hound::WavReader::new(Cursor::new(wav)).expect("readable wav");
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(samples, vec![i16::MAX, -i16::MAX]);
    }

    #[test]
    fn builds_the_transcriptions_url_without_doubling_slashes() {
        assert_eq!(
            transcriptions_url("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1/audio/transcriptions"
        );
        assert_eq!(
            transcriptions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }
}
