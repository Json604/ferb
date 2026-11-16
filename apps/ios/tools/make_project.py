#!/usr/bin/env python3
"""Writes apps/ios/FERB.xcodeproj (the iPhone app's thin Xcode project) from the table below.

Every line of code is in the Swift packages under apps/ios/Packages; the project only makes the two app bundles and
the UI tests from three folders it reads as they are (Xcode's synchronized folders, so adding a file never changes
the project): App/ (the FERB app: one Swift file, its assets and PrivacyInfo.xcprivacy), Gallery/ (the design
gallery, like Android's :gallery) and UITests/. The Info.plist is generated from the build settings below
(INFOPLIST_KEY_*), so the usage strings and the bundle id are here and only here.

The output is deterministic (object ids are hashes of their names), so `--check` fails when the committed project is
not what this script writes: change the table, run it, commit both. No network, no third-party generator.

Usage: python3 -I apps/ios/tools/make_project.py [--check]
"""

import hashlib
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
IOS = os.path.dirname(HERE)
PROJECT = os.path.join(IOS, "FERB.xcodeproj")

# The owner's to change (stage 4: no Apple developer account yet). kb PHN-562.
BUNDLE_ID = "com.ferb.phone"
GALLERY_BUNDLE_ID = BUNDLE_ID + ".gallery"
MIN_IOS = "17.0"  # kb PHN-560; apps/ios/tools/build-rust.sh and the packages say the same
VERSION = "0.28.0"
BUILD = "1"

# The app's Info.plist words (plain, about FERB, kb PHN-567). The camera line is Android's scan_camera_why verbatim.
USAGE = {
    "NSCameraUsageDescription": "FERB uses the camera only to read the code on your Mac. Nothing is recorded.",
    "NSFaceIDUsageDescription": "FERB opens with Face ID, and asks for it again before your Mac goes ahead with something you approve.",
    "NSLocationWhenInUseUsageDescription": "FERB uses your city to get local time and place right. It asks before using your exact location.",
    "NSLocalNetworkUsageDescription": "FERB connects straight to your Mac when both are on the same network, so files and Watch live work at full speed.",
}


def oid(*names):
    return hashlib.sha256("/".join(names).encode()).hexdigest()[:24].upper()


def q(s):
    """A pbxproj string: bare when safe, else quoted."""
    if s and all(c.isalnum() or c in "._/$" for c in s) and not s[0].isdigit():
        return s
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def settings(d, indent="\t\t\t\t"):
    return "".join(f"{indent}{k} = {q(v) if isinstance(v, str) else v};\n" for k, v in sorted(d.items()))


COMMON_TARGET = {
    "CODE_SIGN_STYLE": "Automatic",
    "DEVELOPMENT_TEAM": "",
    "CURRENT_PROJECT_VERSION": BUILD,
    "MARKETING_VERSION": VERSION,
    "IPHONEOS_DEPLOYMENT_TARGET": MIN_IOS,
    "SDKROOT": "iphoneos",
    "SWIFT_VERSION": "5.0",
    "TARGETED_DEVICE_FAMILY": "1",
}

APP_SETTINGS = dict(COMMON_TARGET, **{
    "PRODUCT_BUNDLE_IDENTIFIER": BUNDLE_ID,
    "PRODUCT_NAME": "FERB",
    "ASSETCATALOG_COMPILER_APPICON_NAME": "AppIcon",
    "GENERATE_INFOPLIST_FILE": "YES",
    "INFOPLIST_KEY_CFBundleDisplayName": "FERB",
    "INFOPLIST_KEY_UIApplicationSceneManifest_Generation": "YES",
    "INFOPLIST_KEY_UILaunchScreen_Generation": "YES",
    "INFOPLIST_KEY_UISupportedInterfaceOrientations": "UIInterfaceOrientationPortrait",
    "LD_RUNPATH_SEARCH_PATHS": "$(inherited) @executable_path/Frameworks",
    "DEAD_CODE_STRIPPING": "YES",
    **{"INFOPLIST_KEY_" + k: v for k, v in USAGE.items()},
})

GALLERY_SETTINGS = dict(COMMON_TARGET, **{
    "PRODUCT_BUNDLE_IDENTIFIER": GALLERY_BUNDLE_ID,
    "PRODUCT_NAME": "FERB Gallery",
    "GENERATE_INFOPLIST_FILE": "YES",
    "INFOPLIST_KEY_CFBundleDisplayName": "FERB Gallery",
    "INFOPLIST_KEY_UIApplicationSceneManifest_Generation": "YES",
    "INFOPLIST_KEY_UILaunchScreen_Generation": "YES",
    "INFOPLIST_KEY_UISupportedInterfaceOrientations": "UIInterfaceOrientationPortrait",
    "LD_RUNPATH_SEARCH_PATHS": "$(inherited) @executable_path/Frameworks",
})

UITEST_SETTINGS = dict(COMMON_TARGET, **{
    "PRODUCT_BUNDLE_IDENTIFIER": BUNDLE_ID + ".uitests",
    "PRODUCT_NAME": "$(TARGET_NAME)",
    "GENERATE_INFOPLIST_FILE": "YES",
    "TEST_TARGET_NAME": "FERB",
    "LD_RUNPATH_SEARCH_PATHS": "$(inherited) @executable_path/Frameworks @loader_path/Frameworks",
})

PROJECT_SETTINGS = {
    "Debug": {
        "ALWAYS_SEARCH_USER_PATHS": "NO", "CLANG_ENABLE_MODULES": "YES", "COPY_PHASE_STRIP": "NO",
        "DEBUG_INFORMATION_FORMAT": "dwarf", "ENABLE_STRICT_OBJC_MSGSEND": "YES", "ENABLE_TESTABILITY": "YES",
        "GCC_OPTIMIZATION_LEVEL": "0", "IPHONEOS_DEPLOYMENT_TARGET": MIN_IOS, "ONLY_ACTIVE_ARCH": "YES",
        "SDKROOT": "iphoneos", "SWIFT_ACTIVE_COMPILATION_CONDITIONS": "DEBUG $(inherited)",
        "SWIFT_OPTIMIZATION_LEVEL": "-Onone", "ENABLE_USER_SCRIPT_SANDBOXING": "YES",
    },
    "Release": {
        "ALWAYS_SEARCH_USER_PATHS": "NO", "CLANG_ENABLE_MODULES": "YES", "COPY_PHASE_STRIP": "NO",
        "DEBUG_INFORMATION_FORMAT": "dwarf-with-dsym", "ENABLE_NS_ASSERTIONS": "NO", "ENABLE_STRICT_OBJC_MSGSEND": "YES",
        "IPHONEOS_DEPLOYMENT_TARGET": MIN_IOS, "SDKROOT": "iphoneos", "SWIFT_COMPILATION_MODE": "wholemodule",
        "VALIDATE_PRODUCT": "YES", "ENABLE_USER_SCRIPT_SANDBOXING": "YES",
    },
}

# (target name, product type, product file, folder, settings, package products, depends on)
TARGETS = [
    ("FERB", "com.apple.product-type.application", "FERB.app", "App", APP_SETTINGS, [("FerbApp", "FerbAppUI")], None),
    ("FERB Gallery", "com.apple.product-type.application", "FERB Gallery.app", "Gallery", GALLERY_SETTINGS,
     [("FerbDesign", "FerbGallery")], None),
    ("FERBUITests", "com.apple.product-type.bundle.ui-testing", "FERBUITests.xctest", "UITests", UITEST_SETTINGS, [], "FERB"),
]
PACKAGES = ["FerbApp", "FerbCore", "FerbDesign", "FerbMobile"]  # every local package, so the FERB scheme runs all their tests


def project():
    root = oid("project")
    main_group = oid("group", "main")
    products_group = oid("group", "products")
    packages_group = oid("group", "packages")
    objs = []

    def obj(i, body):
        objs.append((i, body))

    for name, ptype, product, folder, s, deps, depends_on in TARGETS:
        t = oid("target", name)
        obj(oid("product", name), f"{{isa = PBXFileReference; explicitFileType = {q('wrapper.application' if product.endswith('.app') else 'wrapper.cfbundle')}; includeInIndex = 0; path = {q(product)}; sourceTree = BUILT_PRODUCTS_DIR; }}")
        obj(oid("folder", name), f"{{isa = PBXFileSystemSynchronizedRootGroup; path = {q(folder)}; sourceTree = \"<group>\"; }}")
        phases = []
        for kind, isa in (("sources", "PBXSourcesBuildPhase"), ("frameworks", "PBXFrameworksBuildPhase"), ("resources", "PBXResourcesBuildPhase")):
            pid = oid("phase", name, kind)
            files = ""
            if kind == "frameworks":
                files = "".join(f"\n\t\t\t\t{oid('buildfile', name, prod)} /* {prod} */," for _, prod in deps)
            obj(pid, f"{{isa = {isa}; buildActionMask = 2147483647; files = ({files}\n\t\t\t); runOnlyForDeploymentPostprocessing = 0; }}")
            phases.append(pid)
        for pkg, prod in deps:
            obj(oid("dep", name, prod), f"{{isa = XCSwiftPackageProductDependency; package = {oid('package', pkg)}; productName = {q(prod)}; }}")
            obj(oid("buildfile", name, prod), f"{{isa = PBXBuildFile; productRef = {oid('dep', name, prod)}; }}")
        target_deps = ""
        if depends_on:
            proxy = oid("proxy", name, depends_on)
            obj(proxy, f"{{isa = PBXContainerItemProxy; containerPortal = {root}; proxyType = 1; remoteGlobalIDString = {oid('target', depends_on)}; remoteInfo = {q(depends_on)}; }}")
            obj(oid("tdep", name, depends_on), f"{{isa = PBXTargetDependency; target = {oid('target', depends_on)}; targetProxy = {proxy}; }}")
            target_deps = f"\n\t\t\t\t{oid('tdep', name, depends_on)},"
        configs = []
        for c in ("Debug", "Release"):
            cid = oid("config", name, c)
            obj(cid, f"{{isa = XCBuildConfiguration; buildSettings = {{\n{settings(s)}\t\t\t}}; name = {c}; }}")
            configs.append(cid)
        obj(oid("configlist", name), "{isa = XCConfigurationList; buildConfigurations = (" + "".join(f"\n\t\t\t\t{c}," for c in configs) + "\n\t\t\t); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; }")
        product_deps = "".join(f"\n\t\t\t\t{oid('dep', name, prod)}," for _, prod in deps)
        obj(t, "{isa = PBXNativeTarget; buildConfigurationList = " + oid("configlist", name) + "; buildPhases = ("
            + "".join(f"\n\t\t\t\t{p}," for p in phases) + "\n\t\t\t); buildRules = (\n\t\t\t); dependencies = (" + target_deps
            + "\n\t\t\t); fileSystemSynchronizedGroups = (\n\t\t\t\t" + oid("folder", name) + ",\n\t\t\t); name = " + q(name)
            + "; packageProductDependencies = (" + product_deps + "\n\t\t\t); productName = " + q(name)
            + "; productReference = " + oid("product", name) + "; productType = " + q(ptype) + "; }")

    for pkg in PACKAGES:
        obj(oid("package", pkg), f"{{isa = XCLocalSwiftPackageReference; relativePath = {q('Packages/' + pkg)}; }}")
        obj(oid("pkgref", pkg), f"{{isa = PBXFileReference; lastKnownFileType = wrapper; path = {q(pkg)}; sourceTree = \"<group>\"; }}")
    obj(packages_group, "{isa = PBXGroup; children = (" + "".join(f"\n\t\t\t\t{oid('pkgref', p)}," for p in PACKAGES)
        + "\n\t\t\t); path = Packages; sourceTree = \"<group>\"; }")
    obj(products_group, "{isa = PBXGroup; children = (" + "".join(f"\n\t\t\t\t{oid('product', t[0])}," for t in TARGETS)
        + "\n\t\t\t); name = Products; sourceTree = \"<group>\"; }")
    obj(main_group, "{isa = PBXGroup; children = (" + "".join(f"\n\t\t\t\t{oid('folder', t[0])}," for t in TARGETS)
        + f"\n\t\t\t\t{packages_group},\n\t\t\t\t{products_group},\n\t\t\t); sourceTree = \"<group>\"; }}")
    pconfigs = []
    for c, s in PROJECT_SETTINGS.items():
        cid = oid("config", "project", c)
        obj(cid, f"{{isa = XCBuildConfiguration; buildSettings = {{\n{settings(s)}\t\t\t}}; name = {c}; }}")
        pconfigs.append(cid)
    obj(oid("configlist", "project"), "{isa = XCConfigurationList; buildConfigurations = (" + "".join(f"\n\t\t\t\t{c}," for c in pconfigs)
        + "\n\t\t\t); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; }")
    obj(root, "{isa = PBXProject; attributes = {BuildIndependentTargetsInParallel = 1; LastSwiftUpdateCheck = 1600; LastUpgradeCheck = 1600; }; "
        f"buildConfigurationList = {oid('configlist', 'project')}; developmentRegion = en; hasScannedForEncodings = 0; "
        f"knownRegions = (en, Base, ); mainGroup = {main_group}; minimizedProjectReferenceProxies = 1; "
        "packageReferences = (" + "".join(f"\n\t\t\t\t{oid('package', p)}," for p in PACKAGES) + "\n\t\t\t); "
        f"preferredProjectObjectVersion = 77; productRefGroup = {products_group}; projectDirPath = \"\"; projectRoot = \"\"; "
        "targets = (" + "".join(f"\n\t\t\t\t{oid('target', t[0])}," for t in TARGETS) + "\n\t\t\t); }")

    body = "".join(f"\t\t{i} = {b};\n" for i, b in sorted(objs))
    return ("// !$*UTF8*$!\n// Written by apps/ios/tools/make_project.py; change the script, not this file.\n{\n"
            "\tarchiveVersion = 1;\n\tclasses = {\n\t};\n\tobjectVersion = 77;\n\tobjects = {\n" + body
            + f"\t}};\n\trootObject = {root};\n}}\n")


def scheme(name, test_target=None, test_packages=()):
    t = oid("target", name)
    ref = (f'<BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{t}" '
           f'BuildableName = "{name}.app" BlueprintName = "{name}" ReferencedContainer = "container:FERB.xcodeproj"></BuildableReference>')
    tests = ""
    if test_target:
        tt = oid("target", test_target)
        tests += (f'<TestableReference skipped = "NO"><BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{tt}" '
                  f'BuildableName = "{test_target}.xctest" BlueprintName = "{test_target}" ReferencedContainer = "container:FERB.xcodeproj">'
                  '</BuildableReference></TestableReference>\n')
    for pkg, target in test_packages:
        tests += (f'<TestableReference skipped = "NO"><BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{target}" '
                  f'BuildableName = "{target}" BlueprintName = "{target}" ReferencedContainer = "container:Packages/{pkg}">'
                  '</BuildableReference></TestableReference>\n')
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!-- Written by apps/ios/tools/make_project.py -->
<Scheme LastUpgradeVersion = "1600" version = "1.7">
<BuildAction parallelizeBuildables = "YES" buildImplicitDependencies = "YES">
<BuildActionEntries>
<BuildActionEntry buildForTesting = "YES" buildForRunning = "YES" buildForProfiling = "YES" buildForArchiving = "YES" buildForAnalyzing = "YES">
{ref}
</BuildActionEntry>
</BuildActionEntries>
</BuildAction>
<TestAction buildConfiguration = "Debug" selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB" shouldUseLaunchSchemeArgsEnv = "YES">
<Testables>
{tests}</Testables>
</TestAction>
<LaunchAction buildConfiguration = "Debug" selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB" launchStyle = "0" useCustomWorkingDirectory = "NO" ignoresPersistentStateOnLaunch = "NO" debugDocumentVersioning = "YES" debugServiceExtension = "internal" allowLocationSimulation = "YES">
<BuildableProductRunnable runnableDebuggingMode = "0">
{ref}
</BuildableProductRunnable>
</LaunchAction>
<ProfileAction buildConfiguration = "Release" shouldUseLaunchSchemeArgsEnv = "YES" savedToolIdentifier = "" useCustomWorkingDirectory = "NO" debugDocumentVersioning = "YES">
</ProfileAction>
<AnalyzeAction buildConfiguration = "Debug">
</AnalyzeAction>
<ArchiveAction buildConfiguration = "Release" revealArchiveInOrganizer = "YES">
</ArchiveAction>
</Scheme>
"""


def files():
    return {
        "project.pbxproj": project(),
        "xcshareddata/xcschemes/FERB.xcscheme": scheme(
            "FERB", "FERBUITests",
            [("FerbApp", "FerbAppTests"), ("FerbCore", "FerbCoreTests"), ("FerbMobile", "FerbMobileTests"), ("FerbDesign", "FerbDesignTests")]),
        "xcshareddata/xcschemes/FERB Gallery.xcscheme": scheme("FERB Gallery"),
    }


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for rel, text in files().items():
        path = os.path.join(PROJECT, rel)
        old = open(path, encoding="utf-8").read() if os.path.exists(path) else None
        if old == text:
            continue
        if check:
            stale.append(rel)
            continue
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        print(f"wrote FERB.xcodeproj/{rel}")
    if stale:
        print("FERB.xcodeproj differs from apps/ios/tools/make_project.py: " + ", ".join(stale), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
