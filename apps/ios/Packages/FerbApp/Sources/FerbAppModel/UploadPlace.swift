import FerbMobile

/// Where a file or folder sent from the phone is on the Mac, as the owner reads it (kb PHN-608): the Mac's own words for
/// the folder it landed in (`Uploaded.savedIn`, "Downloads › FERB from phone"), else the Mac's upload folder as its
/// settings say (`MacInfo.uploadFolder.shown`), else FERB's default place (tr_place_default). The full path (`saved`,
/// which names the Mac's user folder) is never shown. Android's model/UploadPlace.kt.
public enum UploadPlace {
    public static func shown(_ uploaded: Uploaded) -> String? {
        guard let s = uploaded.savedIn, !s.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return s
    }
}
