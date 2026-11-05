import Foundation

/// What a drag onto FERB may carry (owner, 2026-11-05, D18): files and folders only. Every dragged item must be a file
/// URL and none may be a file promise (Mail attachments, Photos), so selected text, web links and promised files are
/// refused whole and the system slides them back. Structural facts about the pasteboard, never its content.
public enum FileDrop {
    /// The pasteboard type of a file or folder reference (`public.file-url`).
    public static let fileURLType = "public.file-url"
    /// The pasteboard types a file promise carries besides those AppKit lists at run time
    /// (`NSFilePromiseReceiver.readableDraggedTypes`): the legacy promise and the promised file's URL.
    public static let promiseTypes: Set<String> = [
        "com.apple.NSFilePromiseItemMetaData",
        "com.apple.pasteboard.promised-file-url",
        "com.apple.pasteboard.promised-file-content-type",
        "com.apple.pasteboard.promised-suggested-file-name",
        "Apple files promise pasteboard type",
    ]

    /// Whether a drag of these items (each item's pasteboard types) is files and folders only.
    public static func accepts(_ items: [[String]], promises: Set<String> = []) -> Bool {
        let refused = promiseTypes.union(promises)
        return !items.isEmpty && items.allSatisfy { types in
            types.contains(fileURLType) && refused.isDisjoint(with: types)
        }
    }
}
