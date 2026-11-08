import Foundation
import PackagePlugin

/// APP-728 with TEST-508: before the tests compile, SettingsKeyScan lists every UserDefaults key in FERB.app's sources
/// (the FERBNotch and NotchCore targets) as a Swift file of the test target, so the undeclared-preference test reads
/// its own module, never the repository. The same file lists the sources' window and menu presenters (APP-796).
@main
struct SettingsKeysPlugin: BuildToolPlugin {
    func createBuildCommands(context: PluginContext, target: Target) throws -> [Command] {
        let sources = context.package.targets.compactMap { $0 as? SourceModuleTarget }
            .filter { ["FERBNotch", "NotchCore"].contains($0.name) }
            .flatMap { $0.sourceFiles(withSuffix: "swift").map(\.url) }
        let output = context.pluginWorkDirectoryURL.appending(path: "SourceSettingsKeys.swift")
        return [.buildCommand(displayName: "Listing FERB.app's saved preferences",
                              executable: try context.tool(named: "SettingsKeyScan").url,
                              arguments: [output.path] + sources.map(\.path),
                              inputFiles: sources, outputFiles: [output])]
    }
}
