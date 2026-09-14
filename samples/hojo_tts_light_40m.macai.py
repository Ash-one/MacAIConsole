# /// script
# requires-python = ">=3.12,<3.13"
# dependencies = [
#   "numpy>=1.26.4,<3",
#   "onnx>=1.18,<2",
#   "onnxruntime>=1.23,<2",
#   "scipy>=1.14,<2",
#   "soundfile>=0.13,<1",
#   "tokenizers>=0.22,<1",
# ]
#
# [tool.macai]
# schema = "macai.script-runner.v1"
# id = "org.hojoai.hojo-tts-light"
# version = "0.2.0"
# capability = "tts.v1"
# adapter = "hojo-tts-light-onnx"
# model_format = "directory"
# network_during_runtime = false
#
# [tool.macai.timeouts]
# boot_seconds = 30
# load_seconds = 300
# inference_seconds = 600
# shutdown_seconds = 10
#
# [[tool.macai.local_detectors]]
# id = "hojo-tts-light-40m-v2"
# reason = "Hojo-TTS-Light-40M v2 ONNX 模型目录"
# directory_contains = ["Hojo-TTS-Light"]
# required_files = ["Hojo-TTS-Light-40M-llm.onnx", "Hojo-TTS-Light-40M-fine_local.onnx", "Hojo-TTS-Light-40M-decoder.onnx", "Hojo-TTS-Light-40M-voice.npz", "config.json", "tokenizer.json", "tokenizer_config.json"]
#
# [[tool.macai.local_detectors]]
# id = "hojo-tts-light-80m-v2"
# reason = "Hojo-TTS-Light-80M v2 ONNX 模型目录"
# directory_contains = ["Hojo-TTS-Light"]
# required_files = ["Hojo-TTS-Light-llm.onnx", "Hojo-TTS-Light-encoder.onnx", "Hojo-TTS-Light-decoder.onnx", "Hojo-TTS-Light-speaker.onnx", "Hojo-TTS-Light-voice.npz", "config.json", "tokenizer.json", "tokenizer_config.json"]
# ///

from __future__ import annotations

import json
import math
import re
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import soundfile as sf
from onnx import AttributeProto, TensorProto, numpy_helper
from scipy.signal import resample_poly
from tokenizers import Tokenizer


SR = 24_000
CODEC_SR = 16_000
AUDIO_RE = re.compile(r"^\[(\d+)\]$")

F40 = {
    "lm": "Hojo-TTS-Light-40M-llm.onnx",
    "fine": "Hojo-TTS-Light-40M-fine_local.onnx",
    "decoder": "Hojo-TTS-Light-40M-decoder.onnx",
    "voice": "Hojo-TTS-Light-40M-voice.npz",
}

F80 = {
    "lm": "Hojo-TTS-Light-llm.onnx",
    "encoder": "Hojo-TTS-Light-encoder.onnx",
    "decoder": "Hojo-TTS-Light-decoder.onnx",
    "speaker": "Hojo-TTS-Light-speaker.onnx",
    "voice": "Hojo-TTS-Light-voice.npz",
}


def _promote_bf16(model):
    bf16 = TensorProto.BFLOAT16
    fp32 = TensorProto.FLOAT

    def convert_tensor(tensor):
        if tensor.data_type == bf16:
            tensor.CopyFrom(
                numpy_helper.from_array(
                    np.asarray(
                        numpy_helper.to_array(tensor),
                        dtype=np.float32,
                    ),
                    name=tensor.name,
                )
            )

    def convert_graph(graph):
        for tensor in graph.initializer:
            convert_tensor(tensor)

        for value_info in [
            *graph.input,
            *graph.output,
            *graph.value_info,
        ]:
            if (
                value_info.type.HasField("tensor_type")
                and value_info.type.tensor_type.elem_type == bf16
            ):
                value_info.type.tensor_type.elem_type = fp32

        for node in graph.node:
            for attr in node.attribute:
                if (
                    node.op_type == "Cast"
                    and attr.name == "to"
                    and attr.i == bf16
                ):
                    attr.i = fp32

                if attr.type == AttributeProto.TENSOR:
                    convert_tensor(attr.t)
                elif attr.type == AttributeProto.TENSORS:
                    for tensor in attr.tensors:
                        convert_tensor(tensor)
                elif attr.type == AttributeProto.GRAPH:
                    convert_graph(attr.g)
                elif attr.type == AttributeProto.GRAPHS:
                    for subgraph in attr.graphs:
                        convert_graph(subgraph)

        del graph.value_info[:]

    convert_graph(model.graph)
    return model


def _has_bf16(model):
    bf16 = TensorProto.BFLOAT16

    def graph_has_bf16(graph):
        if any(t.data_type == bf16 for t in graph.initializer):
            return True

        for value_info in [
            *graph.input,
            *graph.output,
            *graph.value_info,
        ]:
            if (
                value_info.type.HasField("tensor_type")
                and value_info.type.tensor_type.elem_type == bf16
            ):
                return True

        for node in graph.node:
            for attr in node.attribute:
                if (
                    node.op_type == "Cast"
                    and attr.name == "to"
                    and attr.i == bf16
                ):
                    return True

                if (
                    attr.type == AttributeProto.TENSOR
                    and attr.t.data_type == bf16
                ):
                    return True

                if (
                    attr.type == AttributeProto.TENSORS
                    and any(t.data_type == bf16 for t in attr.tensors)
                ):
                    return True

                if (
                    attr.type == AttributeProto.GRAPH
                    and graph_has_bf16(attr.g)
                ):
                    return True

                if (
                    attr.type == AttributeProto.GRAPHS
                    and any(graph_has_bf16(g) for g in attr.graphs)
                ):
                    return True

        return False

    return graph_has_bf16(model.graph)


def _session(path: Path):
    model = onnx.load(str(path), load_external_data=True)

    if _has_bf16(model):
        source = _promote_bf16(model).SerializeToString()
    else:
        source = str(path)

    options = ort.SessionOptions()
    options.graph_optimization_level = (
        ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    )

    return ort.InferenceSession(
        source,
        sess_options=options,
        providers=["CPUExecutionProvider"],
    )


def _resample(wav, src_sr, dst_sr):
    wav = np.asarray(wav, dtype=np.float32)

    if wav.ndim == 2:
        wav = wav.mean(axis=1)

    if wav.ndim != 1:
        raise ValueError("audio must be mono or multichannel PCM")

    if src_sr == dst_sr:
        return wav

    divisor = math.gcd(int(src_sr), int(dst_sr))

    return resample_poly(
        wav,
        dst_sr // divisor,
        src_sr // divisor,
    ).astype(np.float32)


def _read_audio(path: Path, sample_rate: int):
    wav, source_rate = sf.read(
        str(path),
        dtype="float32",
        always_2d=False,
    )

    return _resample(
        wav,
        int(source_rate),
        sample_rate,
    )


def _mel_filter(
    sample_rate=SR,
    n_fft=1024,
    n_mels=128,
    fmin=0.0,
    fmax=12_000.0,
):
    def hz_to_mel(freq):
        freq = np.asarray(freq, dtype=np.float64)
        mel = freq / (200.0 / 3.0)

        mask = freq >= 1000.0
        mel[mask] = (
            15.0
            + np.log(freq[mask] / 1000.0)
            / (np.log(6.4) / 27.0)
        )

        return mel

    def mel_to_hz(mel):
        mel = np.asarray(mel, dtype=np.float64)
        freq = (200.0 / 3.0) * mel

        mask = mel >= 15.0
        freq[mask] = (
            1000.0
            * np.exp(
                (np.log(6.4) / 27.0)
                * (mel[mask] - 15.0)
            )
        )

        return freq

    fft_hz = np.linspace(
        0.0,
        sample_rate / 2.0,
        n_fft // 2 + 1,
    )

    mel_hz = mel_to_hz(
        np.linspace(
            hz_to_mel([fmin])[0],
            hz_to_mel([fmax])[0],
            n_mels + 2,
        )
    )

    fdiff = np.diff(mel_hz)
    ramps = mel_hz[:, None] - fft_hz[None, :]

    weights = np.zeros(
        (n_mels, len(fft_hz)),
        dtype=np.float64,
    )

    for index in range(n_mels):
        weights[index] = np.maximum(
            0.0,
            np.minimum(
                -ramps[index] / fdiff[index],
                ramps[index + 2] / fdiff[index + 1],
            ),
        )

    weights *= (
        2.0 / (mel_hz[2:] - mel_hz[:-2])
    )[:, None]

    return weights.astype(np.float32)


_MEL = _mel_filter()


def _speaker_mel(wav):
    n_fft = 1024
    hop = 256
    win = 1024
    target = 6 * SR

    wav = np.asarray(wav, dtype=np.float32)[:target]

    if len(wav) < target:
        wav = np.pad(
            wav,
            (0, target - len(wav)),
        )

    pad = (n_fft - hop) // 2

    wav = np.pad(
        wav,
        (pad, pad),
        mode="reflect",
    )

    frames = np.lib.stride_tricks.sliding_window_view(
        wav,
        win,
    )[::hop]

    window = np.hanning(win + 1)[:-1].astype(
        np.float32
    )

    spectrum = np.fft.rfft(
        frames * window[None, :],
        n=n_fft,
        axis=1,
    )

    magnitude = np.sqrt(
        spectrum.real * spectrum.real
        + spectrum.imag * spectrum.imag
        + 1e-9
    ).astype(np.float32)

    mel = np.log(
        np.maximum(
            _MEL @ magnitude.T,
            1e-5,
        )
    )

    return mel.T[None].astype(np.float32)


def _istft(magnitude, phase, n_fft=1920, hop=480):
    magnitude = np.asarray(
        magnitude,
        dtype=np.float32,
    )

    phase = np.asarray(
        phase,
        dtype=np.float32,
    )

    if magnitude.ndim == 3 and magnitude.shape[0] == 1:
        magnitude = magnitude[0]

    if phase.ndim == 3 and phase.shape[0] == 1:
        phase = phase[0]

    if magnitude.ndim != 2 or phase.ndim != 2:
        raise RuntimeError(
            "decoder returned invalid spectrum dimensions"
        )

    if magnitude.shape != phase.shape:
        raise RuntimeError(
            "decoder magnitude and phase shapes differ"
        )

    spectrum = (
        np.maximum(magnitude, 1e-12).clip(max=1e2)
        * np.exp(1j * phase)
    )

    window = np.hanning(n_fft + 1)[:-1].astype(
        np.float32
    )

    frames = (
        np.fft.irfft(
            spectrum,
            n=n_fft,
            axis=0,
        )
        * window[:, None]
    )

    length = (
        (frames.shape[1] - 1) * hop
        + n_fft
    )

    wav = np.zeros(length, dtype=np.float32)
    envelope = np.zeros(length, dtype=np.float32)
    window_squared = window * window

    for index in range(frames.shape[1]):
        start = index * hop

        wav[start : start + n_fft] += frames[:, index]
        envelope[start : start + n_fft] += window_squared

    pad = (n_fft - hop) // 2

    wav = (
        wav[pad:-pad]
        / np.maximum(
            envelope[pad:-pad],
            1e-11,
        )
    )

    return wav.astype(np.float32)


def _sample(
    logits,
    generated,
    temperature,
    top_p,
    repetition_penalty,
    rng,
):
    row = np.asarray(
        logits,
        dtype=np.float32,
    ).copy()

    for token in set(generated):
        value = row[token]

        row[token] = (
            value / repetition_penalty
            if value > 0
            else value * repetition_penalty
        )

    if temperature <= 0:
        return int(row.argmax())

    row = row / temperature
    row -= row.max()

    probabilities = np.exp(row)
    probabilities /= probabilities.sum()

    if top_p < 1.0:
        order = np.argsort(probabilities)[::-1]
        cumulative = np.cumsum(
            probabilities[order]
        )

        keep_count = (
            np.searchsorted(
                cumulative,
                top_p,
                side="right",
            )
            + 1
        )

        keep = order[:keep_count]

        mask = np.zeros_like(
            probabilities,
            dtype=bool,
        )
        mask[keep] = True

        probabilities = np.where(
            mask,
            probabilities,
            0.0,
        )

        probabilities /= probabilities.sum()

    return int(
        rng.choice(
            len(probabilities),
            p=probabilities,
        )
    )


def _audio_table(tokenizer: Tokenizer):
    table = np.full(
        tokenizer.get_vocab_size(),
        -1,
        dtype=np.int64,
    )

    for token_id in range(len(table)):
        decoded = tokenizer.decode(
            [token_id],
            skip_special_tokens=False,
        ).strip()

        match = AUDIO_RE.match(decoded)

        if match:
            table[token_id] = int(
                match.group(1)
            )

    return table


def _audio_positions(
    generated,
    prompt_length,
    speech_end,
    table,
):
    generated = np.asarray(
        generated,
        dtype=np.int64,
    )

    endings = np.flatnonzero(
        generated == speech_end
    )

    sequence = (
        generated[: int(endings[0])]
        if endings.size
        else generated
    )

    if sequence.size == 0:
        return np.empty(
            (0,),
            dtype=np.int64,
        )

    return (
        prompt_length
        + np.flatnonzero(
            table[sequence] >= 0
        )
    )


class HojoModel:
    def __init__(
        self,
        root: Path,
        variant: str,
        profile,
    ):
        self.root = root.resolve()
        self.variant = variant
        self.profile = dict(profile or {})

        files = (
            F40
            if variant == "40M"
            else F80
        )

        self.tok = Tokenizer.from_file(
            str(self.root / "tokenizer.json")
        )

        with open(
            self.root / "config.json",
            encoding="utf-8",
        ) as file:
            self.layers = int(
                json.load(file)[
                    "num_hidden_layers"
                ]
            )

        self.lm = _session(
            self.root / files["lm"]
        )

        output_names = [
            item.name
            for item in self.lm.get_outputs()
        ]

        self.logits_i = output_names.index(
            "logits"
        )

        self.hidden_i = output_names.index(
            "last_hidden_state"
        )

        self.past_i = (
            max(
                self.logits_i,
                self.hidden_i,
            )
            + 1
        )

        self.decoder = _session(
            self.root / files["decoder"]
        )

        self.speech_end = self._token_id(
            "[target_speech_end]"
        )

        self.audio_table = _audio_table(
            self.tok
        )

        with np.load(
            self.root / files["voice"],
            allow_pickle=True,
        ) as data:
            self.embedding = np.asarray(
                data["token_embedding"],
                dtype=np.float32,
            )

            if variant == "40M":
                self.voice_ids = [
                    str(item)
                    for item in data["voice_ids"]
                ]

                self.speaker_embeds = np.asarray(
                    data["speaker_embeds"],
                    dtype=np.float32,
                )

                self.speaker_vecs = np.asarray(
                    data["speaker_vecs"],
                    dtype=np.float32,
                )

        if variant == "40M":
            self.fine = _session(
                self.root / files["fine"]
            )

            self.spk_start = self._token_id(
                "[spk_start]"
            )

        else:
            self.encoder = _session(
                self.root / files["encoder"]
            )

            self.speaker = _session(
                self.root / files["speaker"]
            )

            encoder_input = (
                self.encoder.get_inputs()[0]
            )

            self.enc_name = encoder_input.name

            self.enc_dtype = (
                np.float16
                if "float16"
                in encoder_input.type
                else np.float32
            )

    def _token_id(self, token):
        value = self.tok.token_to_id(token)

        if value is None:
            raise ValueError(
                f"tokenizer missing {token}"
            )

        return int(value)

    def _prompt40(
        self,
        text,
        voice_index,
    ):
        placeholders = "".join(
            f"[spk_emb_{index}]"
            for index in range(16)
        )

        prompt = (
            f"[target_text_start]"
            f"{text}"
            f"[target_text_end]"
            f"[spk_start]"
            f"{placeholders}"
            f"[spk_end]"
            f"[target_speech_start]"
        )

        ids = np.asarray(
            [
                self.tok.encode(
                    prompt,
                    add_special_tokens=True,
                ).ids
            ],
            dtype=np.int64,
        )

        embeddings = self.embedding[
            ids
        ].copy()

        starts = np.flatnonzero(
            ids[0] == self.spk_start
        )

        if starts.size != 1:
            raise RuntimeError(
                "invalid 40M speaker prompt"
            )

        start = int(starts[0]) + 1

        embeddings[
            0,
            start : start + 16,
        ] = self.speaker_embeds[
            voice_index
        ]

        return ids, embeddings

    def _prompt80(
        self,
        text,
        reference_text,
        reference_codes,
    ):
        ref_start = "[ref_speech_start]"
        ref_end = "[ref_speech_end]"
        target_start = "[target_speech_start]"

        if self.tok.token_to_id(
            target_start
        ) is None:
            ref_start = "[speech_start]"
            ref_end = "[speech_end]"
            target_start = "[speech_start]"

        codes = "".join(
            f"[{int(code)}]"
            for code in np.asarray(
                reference_codes
            ).reshape(-1)
        )

        prompt = (
            f"[ref_text_start]"
            f"{reference_text}"
            f"[ref_text_end] "
            f"[target_text_start]"
            f"{text}"
            f"[target_text_end]"
            f"{ref_start}"
            f"{codes}"
            f"{ref_end}"
            f"{target_start}"
        )

        ids = np.asarray(
            [
                self.tok.encode(
                    prompt,
                    add_special_tokens=True,
                ).ids
            ],
            dtype=np.int64,
        )

        return (
            ids,
            self.embedding[
                ids
            ].astype(np.float32),
        )

    def _generate(
        self,
        ids,
        initial_embeddings,
        max_new,
        min_new,
        temperature,
        top_p,
        repetition_penalty,
        seed,
    ):
        empty_cache = {}

        for layer in range(self.layers):
            shape = (1, 1, 0, 128)

            empty_cache[
                f"past_key_values.{layer}.key"
            ] = np.zeros(
                shape,
                dtype=np.float32,
            )

            empty_cache[
                f"past_key_values.{layer}.value"
            ] = np.zeros(
                shape,
                dtype=np.float32,
            )

        outputs = self.lm.run(
            None,
            {
                "inputs_embeds": initial_embeddings,
                "position_ids": np.arange(
                    ids.shape[1],
                    dtype=np.int64,
                )[None],
                **empty_cache,
            },
        )

        logits = outputs[
            self.logits_i
        ]

        hidden = outputs[
            self.hidden_i
        ]

        past = outputs[
            self.past_i :
        ]

        hidden_parts = [hidden]
        generated = []

        rng = np.random.default_rng(seed)

        token = _sample(
            logits[0, -1],
            generated,
            temperature,
            top_p,
            repetition_penalty,
            rng,
        )

        generated.append(token)

        position = ids.shape[1]

        for _ in range(max_new - 1):
            if (
                len(generated) >= min_new
                and token == self.speech_end
            ):
                break

            feed = {
                "inputs_embeds": (
                    self.embedding[
                        np.asarray(
                            [[token]],
                            dtype=np.int64,
                        )
                    ].astype(np.float32)
                ),
                "position_ids": np.asarray(
                    [[position]],
                    dtype=np.int64,
                ),
            }

            for layer in range(
                self.layers
            ):
                feed[
                    f"past_key_values.{layer}.key"
                ] = past[
                    2 * layer
                ]

                feed[
                    f"past_key_values.{layer}.value"
                ] = past[
                    2 * layer + 1
                ]

            outputs = self.lm.run(
                None,
                feed,
            )

            logits = outputs[
                self.logits_i
            ]

            hidden = outputs[
                self.hidden_i
            ]

            past = outputs[
                self.past_i :
            ]

            hidden_parts.append(hidden)

            token = _sample(
                logits[0, -1],
                generated,
                temperature,
                top_p,
                repetition_penalty,
                rng,
            )

            generated.append(token)
            position += 1

        return (
            np.asarray(
                generated,
                dtype=np.int64,
            ),
            np.concatenate(
                hidden_parts,
                axis=1,
            ),
        )

    def generate(
        self,
        text,
        voice,
        prompt_text,
        max_new,
        min_new,
        temperature,
        top_p,
        repetition_penalty,
        seed,
    ):
        if self.variant == "40M":
            if voice not in self.voice_ids:
                raise ValueError(
                    f"unknown voice {voice!r}; "
                    f"available voices: "
                    f"{', '.join(self.voice_ids)}"
                )

            voice_index = (
                self.voice_ids.index(
                    voice
                )
            )

            ids, embeddings = (
                self._prompt40(
                    text,
                    voice_index,
                )
            )

            generated, hidden = (
                self._generate(
                    ids,
                    embeddings,
                    max_new,
                    min_new,
                    temperature,
                    top_p,
                    repetition_penalty,
                    seed,
                )
            )

            positions = _audio_positions(
                generated,
                ids.shape[1],
                self.speech_end,
                self.audio_table,
            )

            if not positions.size:
                raise RuntimeError(
                    "LM generated no audio tokens"
                )

            full_ids = np.concatenate(
                [
                    ids[0],
                    generated,
                ]
            )[None]

            binary_logits = self.fine.run(
                None,
                {
                    "hidden_states": hidden[
                        :,
                        positions - 1,
                    ].astype(np.float32),
                    "coarse_embeddings": (
                        self.embedding[
                            full_ids[
                                :,
                                positions,
                            ]
                        ].astype(np.float32)
                    ),
                    "speaker_embedding": (
                        self.speaker_vecs[
                            voice_index
                            : voice_index + 1
                        ].astype(np.float32)
                    ),
                    "valid_mask": np.ones(
                        (
                            1,
                            len(positions),
                        ),
                        dtype=bool,
                    ),
                },
            )[0]

            bits = np.where(
                binary_logits[0] > 0.0,
                1.0,
                -1.0,
            ).astype(np.float32)

            magnitude, phase = (
                self.decoder.run(
                    None,
                    {
                        "bits": bits,
                    },
                )
            )

            return _istft(
                magnitude,
                phase,
            )

        reference_wav = (
            _safe_relative_wav(
                self.root,
                voice,
            )
        )

        reference_16k = _read_audio(
            reference_wav,
            CODEC_SR,
        ).reshape(
            1,
            1,
            -1,
        ).astype(
            self.enc_dtype
        )

        reference_codes = (
            self.encoder.run(
                None,
                {
                    self.enc_name: reference_16k,
                },
            )[0]
            .reshape(-1)
            .astype(np.int64)
        )

        ids, embeddings = (
            self._prompt80(
                text,
                prompt_text,
                reference_codes,
            )
        )

        reference_24k = _read_audio(
            reference_wav,
            SR,
        )

        speaker_input = (
            self.speaker
            .get_inputs()[0]
            .name
        )

        speaker_vector = (
            self.speaker.run(
                None,
                {
                    speaker_input: (
                        _speaker_mel(
                            reference_24k
                        )
                    )
                },
            )[0]
            .astype(np.float32)
        )

        generated, hidden = (
            self._generate(
                ids,
                embeddings,
                max_new,
                min_new,
                temperature,
                top_p,
                repetition_penalty,
                seed,
            )
        )

        positions = _audio_positions(
            generated,
            ids.shape[1],
            self.speech_end,
            self.audio_table,
        )

        if not positions.size:
            raise RuntimeError(
                "LM generated no audio tokens"
            )

        full_ids = np.concatenate(
            [
                ids[0],
                generated,
            ]
        )[None]

        magnitude, phase = (
            self.decoder.run(
                None,
                {
                    "hidden_states": hidden[
                        :,
                        positions - 1,
                    ].astype(np.float32),
                    "coarse_embeddings": (
                        self.embedding[
                            full_ids[
                                :,
                                positions,
                            ]
                        ].astype(np.float32)
                    ),
                    "speaker_embedding": (
                        speaker_vector
                        .reshape(1, -1)
                        .astype(np.float32)
                    ),
                    "valid_mask": np.ones(
                        (
                            1,
                            len(positions),
                        ),
                        dtype=bool,
                    ),
                },
            )
        )

        return _istft(
            magnitude,
            phase,
        )


def _safe_relative_wav(
    root: Path,
    voice,
):
    if (
        not isinstance(voice, str)
        or not voice.strip()
    ):
        raise ValueError(
            "80M requires voice to be a relative "
            "reference WAV path inside the model directory"
        )

    relative = Path(
        voice.strip()
    )

    if (
        relative.is_absolute()
        or relative.suffix.lower() != ".wav"
    ):
        raise ValueError(
            "80M voice must be a relative .wav path "
            "inside the model directory"
        )

    path = (
        root / relative
    ).resolve()

    try:
        path.relative_to(root)
    except ValueError as exc:
        raise ValueError(
            "voice path escapes model directory"
        ) from exc

    if not path.is_file():
        raise ValueError(
            f"reference WAV not found: {voice}"
        )

    return path


def load(model_path, profile):
    root = Path(model_path)

    if not root.is_dir():
        raise FileNotFoundError(
            f"model directory does not exist: "
            f"{model_path}"
        )

    is_40m = all(
        (root / filename).is_file()
        for filename in F40.values()
    )

    is_80m = all(
        (root / filename).is_file()
        for filename in F80.values()
    )

    if is_40m == is_80m:
        raise ValueError(
            "model directory must contain exactly one "
            "supported Hojo-TTS-Light variant (40M or 80M)"
        )

    return HojoModel(
        root,
        "40M" if is_40m else "80M",
        profile,
    )


def synthesize(
    model,
    text,
    output_path,
    options,
):
    if not isinstance(
        model,
        HojoModel,
    ):
        raise TypeError(
            "invalid Hojo-TTS-Light model"
        )

    if (
        not isinstance(text, str)
        or not text.strip()
    ):
        raise ValueError(
            "text must be non-empty"
        )

    if not isinstance(options, dict):
        raise ValueError(
            "options must be a dictionary"
        )

    output_format = str(
        options.get("format") or "wav"
    ).lower()

    if output_format not in {
        "wav",
        "wave",
        "audio/wav",
        "audio/wave",
        "audio/x-wav",
    }:
        raise ValueError(
            "only WAV output is supported"
        )

    speed = float(
        options.get("speed") or 1.0
    )

    if abs(speed - 1.0) > 1e-6:
        raise ValueError(
            "Hojo-TTS-Light ONNX runner "
            "currently requires speed=1.0"
        )

    language = str(
        options.get("language") or "auto"
    ).lower()

    if language not in {
        "auto",
        "zh",
        "zh-cn",
        "zh-hans",
        "en",
        "en-us",
        "en-gb",
    }:
        raise ValueError(
            "language must be Chinese, English, or auto"
        )

    voice = options.get("voice")

    if (
        model.variant == "40M"
        and (
            not isinstance(voice, str)
            or not voice
        )
    ):
        raise ValueError(
            "40M requires one of its built-in voice IDs"
        )

    prompt_text = (
        options.get("prompt_text")
        or ""
    )

    if not isinstance(
        prompt_text,
        str,
    ):
        raise ValueError(
            "prompt_text must be a string"
        )

    max_new = int(
        options.get(
            "max_new_tokens"
        )
        or 2048
    )

    min_new = int(
        options.get(
            "min_new_tokens"
        )
        if options.get(
            "min_new_tokens"
        ) is not None
        else 10
    )

    temperature = float(
        options.get(
            "temperature"
        )
        if options.get(
            "temperature"
        ) is not None
        else 0.8
    )

    top_p = float(
        options.get(
            "top_p"
        )
        if options.get(
            "top_p"
        ) is not None
        else 0.95
    )

    repetition_penalty = float(
        options.get(
            "repetition_penalty"
        )
        if options.get(
            "repetition_penalty"
        ) is not None
        else 1.1
    )

    seed = int(
        options.get("seed")
        if options.get("seed")
        is not None
        else 42
    )

    if (
        not 1 <= max_new <= 4096
        or not 0 <= min_new <= max_new
    ):
        raise ValueError(
            "invalid min_new_tokens/max_new_tokens"
        )

    if (
        temperature < 0
        or not 0 < top_p <= 1
        or repetition_penalty <= 0
    ):
        raise ValueError(
            "invalid sampling options"
        )

    wav = model.generate(
        text.strip(),
        voice,
        prompt_text,
        max_new,
        min_new,
        temperature,
        top_p,
        repetition_penalty,
        seed,
    )

    sf.write(
        str(output_path),
        np.clip(
            wav,
            -1.0,
            1.0,
        ),
        SR,
        format="WAV",
        subtype="PCM_16",
    )

    return output_path


def describe(model):
    if not isinstance(
        model,
        HojoModel,
    ):
        raise TypeError(
            "invalid Hojo-TTS-Light model"
        )

    info = {
        "adapter": "hojo-tts-light-onnx",
        "variant": model.variant,
        "device": "cpu",
        "sample_rate": SR,
        "languages": [
            "zh",
            "en",
        ],
    }

    if model.variant == "40M":
        info["voices"] = (
            model.voice_ids
        )
    else:
        info["voice_mode"] = (
            "reference_wav"
        )

    return info


def unload(model):
    if not isinstance(
        model,
        HojoModel,
    ):
        return

    for name in (
        "lm",
        "fine",
        "decoder",
        "encoder",
        "speaker",
    ):
        if hasattr(model, name):
            setattr(
                model,
                name,
                None,
            )