import XCTest
import PDFKit
@testable import FERBNotch

final class ExampleDocumentTests: XCTestCase {
    @MainActor
    // decision: APP-573
    func testExampleIsARealSinglePagePDFAndDoesNotReadSources() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let documents = ExampleDocuments(directory: directory)
        let id = UUID()
        let missingSource = URL(fileURLWithPath: "/does-not-exist/Research notes.txt")
        let url = try documents.create(id: id, request: "Summarize these notes — please.", sources: [missingSource])
        let pdf = try XCTUnwrap(PDFDocument(url: url))
        XCTAssertEqual(pdf.pageCount, 1)
        let text = try XCTUnwrap(pdf.string)
        XCTAssertTrue(text.contains("Summarize these notes"))
        XCTAssertTrue(text.contains("Research notes.txt"))
        XCTAssertTrue(text.contains("Interface example"))
        XCTAssertEqual(try documents.create(id: id, request: "Changed", sources: []), url)
        XCTAssertEqual(PDFDocument(url: url)?.string, text)
    }

    @MainActor
    func testDocumentFailureIsReportedWhenDestinationIsAFile() throws {
        let destination = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try Data("occupied".utf8).write(to: destination)
        defer { try? FileManager.default.removeItem(at: destination) }
        XCTAssertThrowsError(try ExampleDocuments(directory: destination).create(id: UUID(), request: "A brief", sources: []))
    }
}
