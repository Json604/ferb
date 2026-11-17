import Foundation

/// What a drag onto FERB may carry (owner, 2026-11-17, APP-930, superseding APP-737's "refused for now"): files and
/// folders, promised files (the screenshot thumbnail after Command-Shift-4 or 5, Mail and Photos) and pictures with no
/// file behind them (an image dragged out of a browser or Preview). Selected text and web links are not files: a drag
/// with any item that is only text or a link is refused whole and the system slides it back (text belongs to the field).
/// Structural facts about the pasteboard types, never their content (APP-931).
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
    /// A web link's types: a link alone is text, not a file.
    public static let linkTypes: Set<String> = ["public.url", "Apple URL pasteboard type"]

    /// How one dragged item reaches FERB.
    public enum Carried: Equatable {
        /// A file or folder already on disk: its path is attached.
        case file
        /// A file its source writes when asked: received into FERB's own folder, then attached.
        case promise
        /// A picture with no file: written as a PNG into FERB's own folder, then attached.
        case image
    }

    /// One item's way in, from its types; nil for text, a link or anything else that isn't a file or a picture. A
    /// promise wins over a file URL beside it (the screenshot thumbnail's file is the source's, not yet the owner's).
    public static func carried(_ types: [String], promises: Set<String> = []) -> Carried? {
        let set = Set(types)
        let promised = !set.isDisjoint(with: promiseTypes.union(promises))
        let picture = !set.isDisjoint(with: ClipboardContent.imageTypes)
        if set.contains(fileURLType) && !promised { return .file }
        // A link is text unless it is a picture's: an image dragged out of a page carries its address beside it
        if !set.isDisjoint(with: linkTypes) && !picture { return nil }
        if promised { return .promise }
        return picture ? .image : nil
    }

    /// Whether a drag of these items (each item's pasteboard types) is files, promised files and pictures only.
    public static func accepts(_ items: [[String]], promises: Set<String> = []) -> Bool {
        !items.isEmpty && items.allSatisfy { carried($0, promises: promises) != nil }
    }
}
