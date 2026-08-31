//! STT 上传音频的进程内归一化（decode-on-ingest）。
//!
//! `/v1/audio/transcriptions` 在边界处把上传音频完整解码为规范的 16-bit PCM
//! WAV 再交给 STT provider。格式裁决只有一个 owner：daemon 入口；provider
//! 契约始终保持"只接收 PCM WAV"。symphonia 为纯 Rust 解码，不引入外部
//! 二进制依赖。转码不重采样、不下混（保原始采样率与声道数）——下游
//! whisper.cpp、Qwen3-ASR 与 sherpa-onnx worker 都自带 16k 单声道重采样，此处无需重复。

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// 归一化失败原因，内容直接作为面向客户端的错误消息。
#[derive(Debug)]
pub struct NormalizeError(pub String);

/// 归一化承诺支持的容器/编解码集合，与 `Cargo.toml` 中 symphonia features
/// 一一对应；opus / HE-AAC / WebM 不在其中（symphonia 未实现）。
pub const SUPPORTED_FORMATS: &str = "wav, mp3, flac, ogg, m4a/aac, alac";

/// 把上传音频完整解码并重写为规范 16-bit PCM WAV（保采样率、保声道）。
///
/// 接收 owned 数据：`MediaSourceStream` 要求 `'static` 音频源，端点处的
/// `Bytes` 可零拷贝转入。`extension` 来自上传文件名，只作为容器探测提示；
/// 探测与解码以文件内容为准。不支持的格式与损坏文件返回
/// [`NormalizeError`]，由调用方映射为 `InvalidRequest`。
pub fn normalize_to_pcm_wav(
    bytes: Vec<u8>,
    extension: Option<&str>,
) -> Result<Vec<u8>, NormalizeError> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = extension {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| {
            NormalizeError(format!(
                "unsupported audio format (supported: {SUPPORTED_FORMATS}); the file may also be corrupt ({error})"
            ))
        })?;
    let mut format = probed.format;

    // mp4/m4a 容器可能同时含视频轨；取第一条能创建解码器的音轨。
    let (track_id, mut decoder) = {
        let codecs = symphonia::default::get_codecs();
        let mut chosen = None;
        for track in format.tracks() {
            if track.codec_params.codec == CODEC_TYPE_NULL {
                continue;
            }
            if let Ok(decoder) = codecs.make(&track.codec_params, &DecoderOptions::default()) {
                chosen = Some((track.id, decoder));
                break;
            }
        }
        chosen.ok_or_else(|| {
            NormalizeError(format!(
                "unsupported audio codec (supported: {SUPPORTED_FORMATS})"
            ))
        })?
    };

    let mut spec = None;
    let mut samples: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => {
                return Err(NormalizeError("audio data is truncated or corrupt".into()));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                if spec.is_none() {
                    spec = Some(*decoded.spec());
                }
                // 每个 packet 按精确尺寸独立转换，不对最大帧数做容量假设。
                let mut converted =
                    SampleBuffer::<i16>::new(decoded.frames() as u64, *decoded.spec());
                converted.copy_interleaved_ref(decoded);
                samples.extend_from_slice(converted.samples());
            }
            // 单个坏包只丢弃该包，尽量产出可用音频。
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(_) => {
                return Err(NormalizeError("audio data is truncated or corrupt".into()));
            }
        }
    }

    let spec = spec.ok_or_else(|| NormalizeError("audio contains no decodable samples".into()))?;
    let channels = u16::try_from(spec.channels.count())
        .map_err(|_| NormalizeError("audio has an unsupported channel count".into()))?;
    if spec.rate == 0 {
        return Err(NormalizeError("audio has an invalid sample rate".into()));
    }
    // WAV chunk 大小上限是 u32；RIFF size 字段还包含 36 字节的既有头。
    let data_len = samples.len() * 2;
    if data_len > (u32::MAX - 36) as usize {
        return Err(NormalizeError("audio is too long for WAV output".into()));
    }
    Ok(encode_pcm_s16_wav(&samples, spec.rate, channels))
}

/// 写入规范小端 RIFF/WAVE：PCM 16-bit、交错声道样本。
/// `wav_duration_ms` 依赖该规范布局解析时长。
fn encode_pcm_s16_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let byte_rate = sample_rate
        .saturating_mul(u32::from(channels))
        .saturating_mul(2);
    let block_align = channels.saturating_mul(2);
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
    out
}

/// 从 RIFF/WAVE 的 `fmt ` 与 `data` chunk 计算输入音频时长。
/// STT endpoint 的输入已是归一化 PCM WAV；解析失败仅省略任务指标，
/// 不影响转写。
pub(crate) fn wav_duration_ms(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut offset = 12usize;
    let mut byte_rate = None;
    let mut data_bytes = None;
    while offset.checked_add(8)? <= bytes.len() {
        let chunk_id = bytes.get(offset..offset + 4)?;
        let chunk_size =
            u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.checked_add(chunk_size)?;
        if chunk_end > bytes.len() {
            return None;
        }

        if chunk_id == b"fmt " && chunk_size >= 12 {
            byte_rate = Some(u32::from_le_bytes(
                bytes
                    .get(chunk_start + 8..chunk_start + 12)?
                    .try_into()
                    .ok()?,
            ));
        } else if chunk_id == b"data" {
            data_bytes = Some(chunk_size as u64);
        }

        offset = chunk_end.checked_add(chunk_size % 2)?;
    }

    let byte_rate = u64::from(byte_rate?);
    let data_bytes = data_bytes?;
    if byte_rate == 0 || data_bytes == 0 {
        return None;
    }
    Some((data_bytes.saturating_mul(1_000) + byte_rate / 2) / byte_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造规范 16-bit PCM WAV，不经过被测代码，避免自证。
    fn raw_pcm_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let data_len = samples.len() * 2;
        let mut wav = Vec::with_capacity(44 + data_len);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * u32::from(channels) * 2).to_le_bytes());
        wav.extend_from_slice(&(channels * 2).to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_len as u32).to_le_bytes());
        wav.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
        wav
    }

    fn wav_header_fields(wav: &[u8]) -> (u16, u32, u32, u32) {
        let channels = u16::from_le_bytes(wav[22..24].try_into().unwrap());
        let sample_rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        (channels, sample_rate, data_len, riff_len)
    }

    #[test]
    fn normalizes_pcm_wav_preserving_rate_channels_and_samples() {
        // 双声道下样本数必须是帧数的整数倍，构造时保持整帧。
        let samples: Vec<i16> = [100, -200, 300, -400, 0, 32767, -32768, 8191].to_vec();
        let input = raw_pcm_wav(&samples, 44_100, 2);
        let output = normalize_to_pcm_wav(input, Some("wav")).unwrap();

        let (channels, sample_rate, data_len, riff_len) = wav_header_fields(&output);
        assert_eq!((channels, sample_rate), (2, 44_100));
        assert_eq!(data_len as usize, samples.len() * 2);
        assert_eq!(riff_len as usize, 36 + data_len as usize);
        let decoded: Vec<i16> = output[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()))
            .collect();
        assert_eq!(decoded, samples);
    }

    #[test]
    fn normalized_output_exposes_duration_metric_and_rejects_garbage() {
        let input = raw_pcm_wav(&vec![0i16; 8_000], 8_000, 1);
        let output = normalize_to_pcm_wav(input, None).unwrap();
        assert_eq!(wav_duration_ms(&output), Some(1_000));
        // 非 WAV 输入没有可解析的时长。
        assert_eq!(wav_duration_ms(b"not a wav"), None);
    }

    #[test]
    fn rejects_garbage_and_empty_input() {
        let error =
            normalize_to_pcm_wav(b"definitely not audio".to_vec(), Some("wav")).unwrap_err();
        assert!(error.0.contains("unsupported audio format"));
        assert!(normalize_to_pcm_wav(Vec::new(), None).is_err());
    }

    #[test]
    fn rejects_truncated_wav_header() {
        let full = raw_pcm_wav(&[42i16; 1_000], 8_000, 1);
        let error = normalize_to_pcm_wav(full[..30].to_vec(), Some("wav")).unwrap_err();
        assert!(error.0.contains("unsupported audio format"));
    }

    // 下方 fixtures 由 scripts/tests/generate_audio_fixtures.py 生成并提交；
    // 它们覆盖"容器探测 + 有损/无损编解码"的完整真实路径，手工构造等价
    // 字节流需要实现整个编码器，不值得。
    const FIXTURE_DURATION_MS: u32 = 500;

    fn assert_fixture_decodes(bytes: &[u8], extension: &str, expected_rate: u32) {
        let output = normalize_to_pcm_wav(bytes.to_vec(), Some(extension)).unwrap();
        let (channels, sample_rate, data_len, _) = wav_header_fields(&output);
        assert_eq!((channels, sample_rate), (1, expected_rate));

        let samples: Vec<i16> = output[44..44 + data_len as usize]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()))
            .collect();
        assert!(!samples.is_empty(), "{extension} produced no samples");
        let rms = (samples
            .iter()
            .map(|s| i64::from(*s) * i64::from(*s))
            .sum::<i64>()
            / samples.len() as i64) as f64;
        // fixtures 是 440 Hz 正弦波；有损编解码后仍须远离静音。
        assert!(rms.sqrt() > 1_000.0, "{extension} output looks silent");
        // 有损编解码有帧边界填充，允许 ~80ms 误差；无损必须精确。
        let duration = wav_duration_ms(&output).unwrap();
        if matches!(extension, "flac" | "wav") {
            assert_eq!(duration, u64::from(FIXTURE_DURATION_MS));
        } else {
            let expected = u64::from(FIXTURE_DURATION_MS);
            assert!(
                duration.abs_diff(expected) <= 80,
                "{extension} duration {duration}ms deviates from {expected}ms"
            );
        }
    }

    #[test]
    fn decodes_mp3_fixture() {
        assert_fixture_decodes(
            include_bytes!("../tests/fixtures/tone-440.mp3"),
            "mp3",
            16_000,
        );
    }

    #[test]
    fn decodes_flac_fixture() {
        assert_fixture_decodes(
            include_bytes!("../tests/fixtures/tone-440.flac"),
            "flac",
            16_000,
        );
    }

    #[test]
    fn decodes_m4a_fixture() {
        assert_fixture_decodes(
            include_bytes!("../tests/fixtures/tone-440.m4a"),
            "m4a",
            16_000,
        );
    }

    #[test]
    fn decodes_ogg_fixture() {
        assert_fixture_decodes(
            include_bytes!("../tests/fixtures/tone-440.ogg"),
            "ogg",
            16_000,
        );
    }

    #[test]
    fn decodes_fixture_without_extension_hint() {
        // 内容嗅探必须独立成立：不提供扩展名提示也要能解码。
        let output = normalize_to_pcm_wav(
            include_bytes!("../tests/fixtures/tone-440.mp3").to_vec(),
            None,
        )
        .unwrap();
        let (_, sample_rate, _, _) = wav_header_fields(&output);
        assert_eq!(sample_rate, 16_000);
    }
}
