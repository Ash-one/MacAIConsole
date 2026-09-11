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

    private func color(at location: Int, in text: NSAttributedString) -> NSColor? {
        text.attribute(.foregroundColor, at: location, effectiveRange: nil) as? NSColor
    }
}
#endif
