// Where this package's design comes from. Not from the lab: the record of the copy.

/// The glass lab (~/Desktop/ferb-glass-lab, read only, never built from here) at the commit this package copies.
public enum LabSource {
    /// The lab commit every copied file and number comes from. Android's lab-numbers.json and sky-golden.json name the
    /// same commit; LabNumbersTests fails when they differ, so the two phones cannot drift apart.
    public static let commit = "23a0fdeabe53a85a124c744b7117c07af58869b5"

    /// SHA-256 of each lab file this package copies or ports, at `commit`. Where Android recorded the same file
    /// (lab-numbers.json `files`) the two must agree.
    public static let files: [String: String] = [
        "Sources/FerbGlass/Theme.swift": "cf890941036c49fa7408fc3f1677441eb7adecb76c35c7fb2e31bbc2f19a5340",
        "Sources/FerbGlass/Sky.swift": "5fa855e9940d718cca048bf076650a117ec6f78644fe3739ffd7e16c74a8ed3c",
        "Sources/FerbGlass/AstraSky.swift": "c960b9bb31d7dd16eeb4e7abaf5560c1ecc6be38a795430ded3137d30362ef7a",
        "Sources/FerbGlass/Data.swift": "537895143296152be4ba85ef88090b2b3171e756c783000c1078f81b36d3d464",
        "Sources/FerbGlassLab/Conversation.swift": "73ea5849ff86e51d6681b003923ac59dadf83e647f1b48792314b88472712874",
        "Sources/FerbGlassLab/Mobile.swift": "b9e93a383b35507eeefaf4761ceca05bcd41bd81609676c394a017d9e7a0e871",
        "Sources/FerbGlassLab/MobileScreens.swift": "058ef86836cf008c69bddf101ff523c2772feed16ac76429479bb342c1d9cdbd",
        "Sources/FerbGlassLab/Weather.swift": "a6a6ab00c47dcbe84f8747f1a7cef5ce2ca285004f61661e4ef9633c17668f80",
        "Sources/FerbGlassLab/Words.swift": "7a4de7cce49c77a234db5f0741faa148585f9380fcb2707629880638474b364f",
        "words/corpus.json": "c65d7ee471d98a4ab40c3a77674a1706bced8fd9e65ea61692baa01738541abc",
        "tokens.json": "28648e8010cb0d0a74fde96a94f65578be0cc79db6caa3483b6173ed1bb42a7a",
    ]
}
