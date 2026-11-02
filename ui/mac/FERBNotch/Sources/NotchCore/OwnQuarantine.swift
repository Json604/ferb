import Darwin
import Foundation

/// APP-601: FERB.app running means the owner already approved it (Open Anyway). Finder copied the disk image's
/// quarantine to every file inside, and FERB's nested code (the background agent, its Python, ferb-secrets, Codex) runs
/// outside Launch Services, where that approval may not reach. So FERB clears the attribute from its own bundle only:
/// every item inside it, links themselves never followed (XATTR_NOFOLLOW; the enumerator doesn't descend into one), the
/// bundle folder last so an interrupted pass runs again at the next launch. Nothing outside the bundle is touched.
/// A translocated copy, a read-only volume (the disk image) or any failure is left as it is, quietly.
public enum OwnQuarantine {
    public static let attribute = "com.apple.quarantine"

    /// Clears the attribute inside `bundle`; returns how many items it was removed from (0 when the bundle carries
    /// none, isn't an app bundle, is translocated or is on a read-only volume).
    @discardableResult
    public static func clear(bundle: URL) -> Int {
        let root = bundle.standardizedFileURL
        guard root.pathExtension == "app", !root.path.contains("/AppTranslocation/"), has(root),
              (try? root.resourceValues(forKeys: [.volumeIsReadOnlyKey]))?.volumeIsReadOnly != true else { return 0 }
        var cleared = 0
        if let items = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil, options: [],
                                                      errorHandler: { _, _ in true }) {
            for case let item as URL in items where remove(item) { cleared += 1 }
        }
        if remove(root) { cleared += 1 }
        return cleared
    }

    /// Whether `url` itself (never a link's target) carries the attribute.
    public static func has(_ url: URL) -> Bool {
        url.withUnsafeFileSystemRepresentation { path in
            guard let path else { return false }
            return getxattr(path, attribute, nil, 0, 0, XATTR_NOFOLLOW) >= 0
        }
    }

    private static func remove(_ url: URL) -> Bool {
        url.withUnsafeFileSystemRepresentation { path in
            guard let path else { return false }
            return removexattr(path, attribute, XATTR_NOFOLLOW) == 0
        }
    }
}
