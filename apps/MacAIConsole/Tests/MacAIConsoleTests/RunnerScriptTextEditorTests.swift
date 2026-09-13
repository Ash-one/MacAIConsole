#if os(macOS)
import AppKit
@testable import MacAIConsole
import XCTest

final class RunnerScriptTextEditorTests: XCTestCase {
    func testHighlightsMetadataAndPythonSyntax() throws {
        let source = "# id = \"org.example\"\ndef chat():\n    return \"hello\""
        let rendered = RunnerScriptTextEditor.highlightedSource(source)
        let string = source as NSString

        XCTAssertEqual(color(at: string.range(of: "id").location, in: rendered), .systemBlue)
        XCTAssertEqual(color(at: string.range(of: "def").location, in: rendered), .systemPurple)
        XCTAssertEqual(color(at: string.range(of: "chat").location, in: rendered), .systemTeal)
        XCTAssertEqual(color(at: string.range(of: "\"hello\"").location, in: rendered), .systemRed)
    }

    func testDependencyAssistantReplacesOnlyTheActiveMetadataLine() {
        let source = "# # dependencies = [\"example\"]\n# dependencies = []\n\ndef load(): pass"
        XCTAssertEqual(
            ScriptRunnerEditorSheet.replacingDependencies(
                in: source,
                with: ["mlx-lm", "sentencepiece"]
            ),
            "# # dependencies = [\"example\"]\n# dependencies = [\"mlx-lm\",\"sentencepiece\"]\n\ndef load(): pass"
        )
    }

    func testUserModifiedGuardTreatsMissingTemplateBaselineAsModified() {
        XCTAssertFalse(
            ScriptRunnerEditorSheet.isEditorContentUserModified(source: "", loadedTemplate: nil)
        )
        XCTAssertFalse(
            ScriptRunnerEditorSheet.isEditorContentUserModified(source: "tpl", loadedTemplate: "tpl")
        )
        XCTAssertTrue(
            ScriptRunnerEditorSheet.isEditorContentUserModified(
                source: "edited", loadedTemplate: "template"
            )
        )
        XCTAssertTrue(
            ScriptRunnerEditorSheet.isEditorContentUserModified(source: "typed", loadedTemplate: nil)
        )
    }

    func testPEP723MetadataBlockDetectionMatchesDaemonOpenerRule() {
        let withBlock = "# /// script\n# requires-python = \">=3.12,<3.13\"\n# ///\n\ndef load(): pass"
        XCTAssertTrue(ScriptRunnerEditorSheet.hasPEP723MetadataBlock(withBlock))
        XCTAssertFalse(ScriptRunnerEditorSheet.hasPEP723MetadataBlock("# /// scriptx\n"))
        XCTAssertFalse(ScriptRunnerEditorSheet.hasPEP723MetadataBlock("import numpy as np\n"))
    }

    func testAIPromptEmbedsTemplateConfirmationStepAndCapabilityHook() {
        let template = """
        # /// script
        # requires-python = ">=3.12,<3.13"
        # dependencies = []
        # [tool.macai]
        # capability = "tts.v1"
        # ///
        def load(model_path, profile):
            return None
        """
        let prompt = ScriptRunnerAIPrompt.makeAIPrompt(kind: "tts", template: template)

        XCTAssertTrue(prompt.contains(template), "官方模板原文必须完整注入")
        XCTAssertTrue(prompt.contains("macai.script-runner.v1"))
        XCTAssertTrue(prompt.contains("语音合成（tts.v1）"))
        XCTAssertTrue(prompt.contains("synthesize(model, text, output_path, options)"))
        XCTAssertTrue(prompt.contains("索要模型的网址"), "缺少模型信息时必须先询问网址")
    }

    func testAIPromptMatchesHookPerCapability() {
        let expectedHooks = [
            "chat": "chat(model, messages, options)",
            "stt": "transcribe(model, wav_path, options)",
            "tts": "synthesize(model, text, output_path, options)",
        ]
        for (kind, hook) in expectedHooks {
            let prompt = ScriptRunnerAIPrompt.makeAIPrompt(kind: kind, template: "# /// script\n# ///\n")
            XCTAssertTrue(prompt.contains(hook), "kind=\(kind) 的 prompt 必须声明 \(hook)")
        }
    }

    private func color(at location: Int, in text: NSAttributedString) -> NSColor? {
        text.attribute(.foregroundColor, at: location, effectiveRange: nil) as? NSColor
    }
}
#endif
