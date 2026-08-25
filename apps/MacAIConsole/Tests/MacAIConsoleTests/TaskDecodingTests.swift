#if canImport(XCTest)
import XCTest
@testable import MacAIConsole

final class TaskDecodingTests: XCTestCase {
    func testDecodesAllTaskKindsAndStatuses() throws {
        let data = Data(
            #"""
            {
              "running": [
                {
                  "id": "req_chat",
                  "kind": "chat",
                  "status": "running",
                  "model": "qwen3",
                  "provider": "llama.cpp",
                  "input_preview": "请总结",
                  "started_at_ms": 1787670000000,
                  "completed_at_ms": null,
                  "duration_ms": null,
                  "error": null
                },
                {
                  "id": "req_future",
                  "kind": "image_generation",
                  "status": "queued",
                  "model": "future",
                  "input_preview": "未知",
                  "started_at_ms": 1787670000001
                }
              ],
              "completed": [
                {
                  "id": "req_stt",
                  "kind": "stt",
                  "status": "succeeded",
                  "model": "whisper-base",
                  "provider": "whisper.cpp",
                  "input_preview": "audio.wav",
                  "started_at_ms": 1787670001000,
                  "completed_at_ms": 1787670002000,
                  "duration_ms": 1000,
                  "error": null
                },
                {
                  "id": "req_tts",
                  "kind": "tts",
                  "status": "failed",
                  "model": "kokoro",
                  "input_preview": "你好",
                  "started_at_ms": 1787670003000,
                  "completed_at_ms": 1787670005000,
                  "duration_ms": 2000,
                  "error": "worker failed"
                },
                {
                  "id": "req_cancelled",
                  "kind": "chat",
                  "status": "cancelled",
                  "model": "qwen3",
                  "input_preview": "中断",
                  "started_at_ms": 1787670006000,
                  "completed_at_ms": 1787670006500,
                  "duration_ms": 500,
                  "error": "客户端连接已中断"
                }
              ]
            }
            """#.utf8
        )

        let list = try JSONDecoder().decode(InferenceTaskList.self, from: data)

        XCTAssertEqual(list.running.count, 2)
        XCTAssertEqual(list.completed.count, 3)
        XCTAssertEqual(list.running[0].startedAtMs, 1_787_670_000_000)
        XCTAssertTrue(list.running[0].isRunning)
        XCTAssertEqual(list.running[1].kind, "image_generation")
        XCTAssertEqual(list.running[1].statusTitle, "queued")
        XCTAssertEqual(list.completed[0].kindTitle, "语音识别")
        XCTAssertEqual(list.completed[1].statusTitle, "失败")
        XCTAssertEqual(list.completed[2].statusTitle, "已中断")
        XCTAssertNil(list.completed[1].provider)
    }

    func testDecodesChatDetailWithUsageAndNilOptionalFields() throws {
        let data = Data(
            #"""
            {
              "id": "req_chat",
              "kind": "chat",
              "status": "succeeded",
              "model": "qwen3",
              "provider": "llama.cpp",
              "started_at_ms": 1787670000000,
              "completed_at_ms": 1787670002418,
              "duration_ms": 2418,
              "request": {
                "messages": [
                  {"role": "user", "content": "请总结这段内容"}
                ],
                "input_text": null,
                "file_name": null,
                "file_size_bytes": null,
                "language": null,
                "voice": null,
                "format": null,
                "speed": null,
                "stream": true,
                "temperature": 0.7,
                "max_tokens": 512
              },
              "result": {
                "output_text": "总结结果",
                "language": null,
                "finish_reason": "stop",
                "prompt_tokens": 12,
                "completion_tokens": 8,
                "total_tokens": 20,
                "content_type": null,
                "byte_count": null
              },
              "request_truncated": false,
              "result_truncated": false,
              "error": null
            }
            """#.utf8
        )

        let detail = try JSONDecoder().decode(InferenceTaskDetail.self, from: data)

        XCTAssertEqual(detail.request?.messages?.first?.role, "user")
        XCTAssertEqual(detail.request?.stream, true)
        XCTAssertEqual(detail.request?.temperature, 0.7)
        XCTAssertEqual(detail.request?.maxTokens, 512)
        XCTAssertEqual(detail.result?.outputText, "总结结果")
        XCTAssertEqual(detail.result?.totalTokens, 20)
        XCTAssertEqual(detail.summary.inputPreview, "请总结这段内容")
        XCTAssertEqual(detail.durationMs, 2418)
        XCTAssertFalse(detail.requestTruncated)
        XCTAssertFalse(detail.resultTruncated)
    }

    func testDecodesSTTAndTTSTaskDetailsWithoutAudioPayload() throws {
        let data = Data(
            #"""
            {
              "id": "req_stt",
              "kind": "stt",
              "status": "succeeded",
              "model": "whisper-base",
              "provider": "whisper.cpp",
              "started_at_ms": 1,
              "completed_at_ms": 2,
              "duration_ms": 1,
              "request": {
                "messages": [],
                "file_name": "meeting.wav",
                "file_size_bytes": 4096,
                "language": "zh",
                "format": "json"
              },
              "result": {
                "output_text": "你好",
                "language": "zh"
              },
              "request_truncated": false,
              "result_truncated": false
            }
            """#.utf8
        )
        let stt = try JSONDecoder().decode(InferenceTaskDetail.self, from: data)
        XCTAssertEqual(stt.request?.fileName, "meeting.wav")
        XCTAssertEqual(stt.request?.fileSizeBytes, 4096)
        XCTAssertEqual(stt.result?.outputText, "你好")

        let ttsData = Data(
            #"""
            {
              "id": "req_tts",
              "kind": "tts",
              "status": "succeeded",
              "model": "kokoro",
              "started_at_ms": 1,
              "completed_at_ms": 2,
              "duration_ms": 1,
              "request": {
                "messages": [],
                "input_text": "朗读我",
                "voice": "zf_001",
                "format": "wav",
                "speed": 1.0
              },
              "result": {
                "content_type": "audio/wav",
                "byte_count": 8192
              },
              "request_truncated": false,
              "result_truncated": false
            }
            """#.utf8
        )
        let tts = try JSONDecoder().decode(InferenceTaskDetail.self, from: ttsData)
        XCTAssertEqual(tts.request?.inputText, "朗读我")
        XCTAssertEqual(tts.result?.contentType, "audio/wav")
        XCTAssertEqual(tts.result?.byteCount, 8192)
        XCTAssertNil(tts.result?.outputText)
    }
}
#endif
