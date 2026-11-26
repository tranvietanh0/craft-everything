//! Building an Android rebuild ([`stacks::Stack::android`]): the Gradle wrapper's `assembleDebug`,
//! which passes only when it leaves a debug APK. The JDK and Android SDK are found here, not on
//! PATH alone: Craft Anything often runs in WSL with neither, while Windows has both, so from WSL
//! the build runs Windows' own through `cmd.exe` (WSL interop) in the same folder.

use super::*;
use crate::cli_agents::herdr::paths::{linux_to_windows, windows_to_linux};
use std::ffi::OsString;

/// Where the debug APK is, under the Gradle project (app/).
pub const APK_DIR: &str = "app/build/outputs/apk/debug";

/// Settings that name the Android SDK, the first that is a folder wins.
const SDK_ENV: [&str; 3] = ["CRAFT_ANDROID_SDK", "ANDROID_HOME", "ANDROID_SDK_ROOT"];
/// A JDK for the build, passed to Gradle as JAVA_HOME (its own JAVA_HOME is used otherwise).
const JAVA_ENV: &str = "CRAFT_JAVA_HOME";
/// Where an Android SDK is installed when no setting names one, below a Windows drive or the home
/// folder; `*` is any one folder (Unity's editor version, the Windows user).
const SDK_GLOBS: [&str; 4] = [
    "Program Files/Unity/Hub/Editor/*/Editor/Data/PlaybackEngines/AndroidPlayer/SDK",
    "Users/*/AppData/Local/Android/Sdk",
    "Android/Sdk",
    "Library/Android/sdk",
];
/// Gradle's minutes: the first build downloads Gradle, the Android plugin and the libraries.
const MINUTES: u64 = 30;

/// How the build is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// This system's own JDK runs the wrapper.
    Native,
    /// From WSL, Windows' JDK runs `gradlew.bat` through `cmd.exe`.
    Windows,
}

/// A setting by name, None when unset or blank.
type Setting = Box<dyn Fn(&str) -> Option<String>>;

/// What this computer has, read once; tests fill it in themselves.
pub struct Probe {
    /// Running in WSL (Windows' programs may be reachable through interop).
    pub wsl: bool,
    /// Running on Windows itself: the wrapper is gradlew.bat.
    pub windows: bool,
    /// Settings by name (the process environment).
    pub env: Setting,
    /// Whether a program is on PATH.
    pub have: Box<dyn Fn(&str) -> bool>,
    /// Folders [`SDK_GLOBS`] are looked for below (a Windows drive, the home folder).
    pub roots: Vec<PathBuf>,
}

impl Probe {
    pub fn current() -> Probe {
        let wsl = cfg!(unix) && std::env::var("WSL_DISTRO_NAME").is_ok_and(|d| !d.trim().is_empty());
        let mut roots = Vec::new();
        if wsl {
            roots.push(PathBuf::from("/mnt/c"));
        } else if cfg!(windows) {
            roots.push(PathBuf::from(format!("{}\\", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()))));
        }
        roots.extend(dirs::home_dir());
        Probe {
            wsl,
            windows: cfg!(windows),
            env: Box::new(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty())),
            have: Box::new(|p| craft_anything_platform::which(p).is_some()),
            roots,
        }
    }

    /// A path a setting names, as this process reads it (a Windows path from WSL is mapped).
    fn path(&self, value: &str) -> PathBuf {
        let mapped = if self.wsl { windows_to_linux(value) } else { None };
        PathBuf::from(mapped.unwrap_or_else(|| value.to_string()))
    }
}

/// The SDK found and how the build runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub route: Route,
    /// The SDK as this process reads it; None when app/local.properties names one itself.
    pub sdk: Option<PathBuf>,
    /// A JDK the person named ([`JAVA_ENV`]).
    pub java: Option<PathBuf>,
}

/// A missing tool, as an error that isn't blamed on the build: what is missing and how to get it.
fn missing_tool(what: String, fix: String) -> ToolError {
    ToolError::new(ErrorCode::EngineMissing, what).with_remediation(fix).with_details(serde_json::json!({"reason": "not_found"}))
}

/// Folders matching `pattern` below `root`, each `*` being any one folder.
fn expand(root: &Path, pattern: &str) -> Vec<PathBuf> {
    pattern.split('/').fold(vec![root.to_path_buf()], |dirs, part| {
        dirs.into_iter()
            .flat_map(|d| match part {
                "*" => std::fs::read_dir(&d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect(),
                _ => vec![d.join(part)],
            })
            .collect()
    })
}

/// An Android SDK: it has platforms and build tools.
fn is_sdk(dir: &Path) -> bool {
    dir.join("platforms").is_dir() && dir.join("build-tools").is_dir()
}

/// The SDK installed where [`SDK_GLOBS`] look, the most recently changed first (a newer Unity).
fn installed_sdk(roots: &[PathBuf]) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = roots.iter().flat_map(|r| SDK_GLOBS.iter().flat_map(move |g| expand(r, g))).filter(|d| is_sdk(d)).collect();
    found.sort_by_key(|d| std::cmp::Reverse(std::fs::metadata(d).and_then(|m| m.modified()).ok()));
    found.into_iter().next()
}

/// The JDK Unity ships beside its SDK (`AndroidPlayer/OpenJDK`), to name in a fix.
fn unity_jdk(sdk: Option<&Path>) -> Option<PathBuf> {
    sdk?.parent().map(|p| p.join("OpenJDK")).filter(|j| j.join("bin").is_dir())
}

/// Whether app/local.properties names the SDK itself (`sdk.dir=`), which Gradle reads first.
fn names_its_sdk(app: &Path) -> bool {
    std::fs::read_to_string(app.join("local.properties")).is_ok_and(|t| t.lines().any(|l| l.trim_start().starts_with("sdk.dir")))
}

const SDK_FIX: &str = "Install the Android SDK with its platforms and build tools (Android Studio, the Android command-line tools with `sdkmanager \"platforms;android-35\" \"build-tools;35.0.0\"`, or Unity Hub's Android Build Support module), then try again; set CRAFT_ANDROID_SDK to its folder if it isn't in Unity Hub's editors or Android Studio's usual folder.";

/// How `app` is built here, or what is missing.
pub fn plan(app: &Path, probe: &Probe) -> Result<Plan, ToolError> {
    let set = |k: &str| (probe.env)(k).map(|v| probe.path(&v));
    let sdk = SDK_ENV.iter().find_map(|k| set(k).filter(|d| d.is_dir())).or_else(|| installed_sdk(&probe.roots));
    if sdk.is_none() && !names_its_sdk(app) {
        return Err(missing_tool("No Android SDK found, so the Android app can't be built.".into(), SDK_FIX.into()));
    }
    let java = set(JAVA_ENV).filter(|d| d.is_dir());
    let has_java = java.is_some() || set("JAVA_HOME").is_some_and(|d| d.is_dir()) || (probe.have)("java");
    // A Linux JDK in WSL builds with a Linux SDK only; one on a Windows drive is Windows' own.
    let linux_sdk = sdk.as_ref().is_none_or(|d| !d.starts_with("/mnt/"));
    let route = if probe.wsl && !(has_java && linux_sdk) {
        if !(probe.have)("cmd.exe") {
            return Err(missing_tool(
                "No JDK in WSL, and Windows can't be reached from it to build with Windows' own.".into(),
                "Turn WSL's Windows interop back on (so cmd.exe runs) and install JDK 17 on Windows; or install JDK 17 (the openjdk-17-jdk package) and the Android command-line tools in WSL and set ANDROID_HOME to them.".into(),
            ));
        }
        if !absolute(app).starts_with("/mnt/") {
            return Err(missing_tool(
                format!("{} is inside WSL, where Windows' Gradle can't build it.", app.display()),
                "Keep Android projects on a Windows drive (under /mnt/c), or install JDK 17 and the Android SDK in WSL.".into(),
            ));
        }
        Route::Windows
    } else if has_java {
        Route::Native
    } else {
        let bundled = unity_jdk(sdk.as_deref()).map(|j| format!(" Unity's own is at {}.", j.display())).unwrap_or_default();
        return Err(missing_tool(
            "No JDK found (no java on PATH, no JAVA_HOME), so the Android app can't be built.".into(),
            format!("Install JDK 17 (for example Eclipse Temurin 17) and set JAVA_HOME, or set CRAFT_JAVA_HOME to a JDK 17 folder.{bundled}"),
        ));
    };
    Ok(Plan { route, sdk, java })
}

/// What the build is told, as a Windows program reads it on the Windows route.
fn settings(plan: &Plan, probe: &Probe) -> Vec<(&'static str, OsString)> {
    let distro = (probe.env)("WSL_DISTRO_NAME").unwrap_or_default();
    let seen = |p: &Path| match plan.route {
        Route::Windows => linux_to_windows(&p.display().to_string(), &distro),
        Route::Native => p.display().to_string(),
    };
    let mut vars: Vec<(&'static str, OsString)> = Vec::new();
    if let Some(sdk) = &plan.sdk {
        vars.push(("ANDROID_HOME", seen(sdk).into()));
    }
    if let Some(java) = &plan.java {
        vars.push(("JAVA_HOME", seen(java).into()));
    }
    if plan.route == Route::Windows {
        // WSL gives a Windows program only the settings WSLENV lists (the rest are Windows' own).
        let listed = std::iter::once((probe.env)("WSLENV").unwrap_or_default()).chain(vars.iter().map(|(k, _)| k.to_string()));
        let wslenv = listed.filter(|s| !s.is_empty()).collect::<Vec<_>>().join(":");
        vars.push(("WSLENV", wslenv.into()));
    }
    vars
}

/// The debug APK Gradle left in `app`, if there is one.
pub fn debug_apk(app: &Path) -> Option<PathBuf> {
    let dir = app.join(APK_DIR);
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("apk")))
}

/// Build `app` with its Gradle wrapper's `assembleDebug`: passes only when Gradle succeeds and
/// leaves a debug APK in [`APK_DIR`]. A missing JDK or SDK is an error naming what to install.
pub fn build(app: &Path, place: &str, probe: &Probe, stop: &CheckStop, env: &[(&str, &OsStr)]) -> Result<(bool, String), ToolError> {
    let plan = plan(app, probe)?;
    let bat = plan.route == Route::Windows || probe.windows;
    let wrapper = if bat { "gradlew.bat" } else { "gradlew" };
    if !app.join(wrapper).is_file() {
        return Ok((false, format!("No Gradle wrapper ({wrapper}) in {place} yet: check in gradlew, gradlew.bat and gradle/wrapper/, and build with it.")));
    }
    let (program, mut args) = match (plan.route, bat) {
        // The folder is on a Windows drive, so cmd.exe starts in it. `.\`: with
        // NoDefaultCurrentDirectoryInExePath set, cmd.exe doesn't look in its folder for a bare name.
        (Route::Windows, _) => (PathBuf::from("cmd.exe"), vec!["/c".to_string(), format!(".\\{wrapper}")]),
        (Route::Native, true) => (PathBuf::from(absolute(&app.join(wrapper))), Vec::new()),
        // Through sh: a wrapper written without its executable bit still runs.
        (Route::Native, false) => (PathBuf::from("sh"), vec![absolute(&app.join(wrapper))]),
    };
    args.extend(["assembleDebug", "--console=plain"].map(String::from));
    let ours = settings(&plan, probe);
    let all: Vec<(&str, &OsStr)> = env.iter().copied().chain(ours.iter().map(|(k, v)| (*k, v.as_os_str()))).collect();
    match run_in(app, &program, &args, MINUTES, stop, &all) {
        Ok((ok, out)) => judge(app, place, ok, &out, plan.sdk.as_deref()),
        Err(e) => step_error("build (`gradlew assembleDebug`)", e),
    }
}

/// Whether a finished Gradle run built the app: it succeeded and a debug APK is there. A JDK or
/// SDK Gradle couldn't find is a missing tool, not a failed build.
fn judge(app: &Path, place: &str, ok: bool, out: &str, sdk: Option<&Path>) -> Result<(bool, String), ToolError> {
    if out.contains("JAVA_HOME is not set") || out.contains("JAVA_HOME is set to an invalid directory") {
        let bundled = unity_jdk(sdk).map(|j| format!(" Unity's own is at {}.", j.display())).unwrap_or_default();
        return Err(missing_tool(
            "Gradle found no JDK, so the Android app can't be built.".into(),
            format!("Install JDK 17 (for example Eclipse Temurin 17) and set JAVA_HOME, or set CRAFT_JAVA_HOME to a JDK 17 folder.{bundled}"),
        ));
    }
    if out.contains("SDK location not found") {
        return Err(missing_tool("Gradle found no Android SDK.".into(), SDK_FIX.into()));
    }
    match (ok, debug_apk(app)) {
        (true, Some(apk)) => Ok((true, format!("Build succeeded: {}.", apk.strip_prefix(app).unwrap_or(&apk).display()))),
        (true, None) => Ok((
            false,
            format!("Gradle finished, but there's no debug APK in {APK_DIR}/ in {place}: the app module must be app/app/, built by `assembleDebug`."),
        )),
        (false, _) => Ok((false, format!("Build failed (`gradlew assembleDebug`): {}", error_lines(out)))),
    }
}

fn absolute(app: &Path) -> String {
    craft_anything_engine::workspace::absolute(app).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A computer with these settings and programs, looking for SDKs below `roots`.
    fn probe(wsl: bool, env: &[(&str, &str)], have: &[&str], roots: Vec<PathBuf>) -> Probe {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let have: Vec<String> = have.iter().map(|s| s.to_string()).collect();
        Probe {
            wsl,
            windows: false,
            env: Box::new(move |k| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())),
            have: Box::new(move |p| have.iter().any(|h| h == p)),
            roots,
        }
    }

    fn sdk_at(dir: &Path) -> PathBuf {
        for sub in ["platforms/android-35", "build-tools/35.0.0"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        dir.to_path_buf()
    }

    fn missing_reason(err: &ToolError) -> bool {
        is_unavailable_tool_error(err) && err.remediation.as_deref().is_some_and(|r| r.contains("Install"))
    }

    /// No SDK anywhere is a missing tool that names what to install, never a pass or a build
    /// failure; a JDK missing too, once the SDK is there, names JDK 17.
    #[test]
    fn a_missing_sdk_or_jdk_names_what_to_install() {
        let (app, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let err = plan(app.path(), &probe(false, &[], &["java"], vec![root.path().into()])).unwrap_err();
        assert!(missing_reason(&err) && err.remediation.as_deref().is_some_and(|r| r.contains("Android SDK")), "{err:?}");
        let sdk = sdk_at(&root.path().join("sdk"));
        let env = [("CRAFT_ANDROID_SDK", sdk.to_str().unwrap())];
        let err = plan(app.path(), &probe(false, &env, &[], vec![])).unwrap_err();
        assert!(missing_reason(&err) && err.remediation.as_deref().is_some_and(|r| r.contains("JDK 17")), "{err:?}");
        let built = build(app.path(), "app/", &probe(false, &env, &[], vec![]), &CheckStop::default(), &[]);
        assert!(built.is_err_and(|e| missing_reason(&e)), "the build check stops on it too");
        assert_eq!(plan(app.path(), &probe(false, &env, &["java"], vec![])).map(|p| p.route).ok(), Some(Route::Native));
    }

    /// Unity Hub's bundled SDK is found under any editor version, without a setting.
    #[test]
    fn an_sdk_in_unity_hub_is_found_by_its_folder_whatever_the_version() {
        let (app, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let sdk = sdk_at(&root.path().join("Program Files/Unity/Hub/Editor/9999.1.2f3/Editor/Data/PlaybackEngines/AndroidPlayer/SDK"));
        std::fs::create_dir_all(root.path().join("Program Files/Unity/Hub/Editor/2022.3.1f1/Editor")).unwrap();
        let found = plan(app.path(), &probe(false, &[], &["java"], vec![root.path().into()])).unwrap();
        assert_eq!(found.sdk, Some(sdk));
    }

    /// In WSL with no JDK, the build goes through Windows when the project is on a Windows drive,
    /// passing the SDK as a Windows path that WSLENV hands over; inside WSL it can't.
    #[test]
    fn wsl_without_a_jdk_builds_through_windows_on_a_windows_drive() {
        let sdk = tempfile::tempdir().unwrap();
        let sdk = sdk_at(sdk.path());
        let env = [("ANDROID_HOME", sdk.to_str().unwrap()), ("WSLENV", "WT_SESSION")];
        let app = tempfile::tempdir().unwrap();
        let err = plan(app.path(), &probe(true, &env, &["cmd.exe"], vec![])).unwrap_err();
        assert!(is_unavailable_tool_error(&err) && err.message.contains("inside WSL"), "{err:?}");
        let no_interop = plan(app.path(), &probe(true, &env, &[], vec![])).unwrap_err();
        assert!(is_unavailable_tool_error(&no_interop) && no_interop.message.contains("No JDK"), "{no_interop:?}");
        let windows = Plan { route: Route::Windows, sdk: Some("/mnt/c/Android/Sdk".into()), java: None };
        let vars = settings(&windows, &probe(true, &env, &[], vec![]));
        let get = |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string_lossy().into_owned());
        assert_eq!(get("ANDROID_HOME").as_deref(), Some("C:\\Android\\Sdk"));
        assert!(get("WSLENV").is_some_and(|w| w.contains("WT_SESSION") && w.contains("ANDROID_HOME")), "{vars:?}");
    }

    /// Live: the Gradle project at CRAFT_ANDROID_LIVE_APP builds with this computer's own JDK and
    /// SDK as found (from WSL, through Windows) and leaves a debug APK.
    #[test]
    #[ignore = "builds a real Android project; set CRAFT_ANDROID_LIVE_APP to its folder"]
    fn a_real_android_project_builds_a_debug_apk() {
        let app = PathBuf::from(std::env::var("CRAFT_ANDROID_LIVE_APP").unwrap_or_default());
        let found = plan(&app, &Probe::current());
        eprintln!("plan: {found:?}");
        let (passed, note) = build(&app, "app/", &Probe::current(), &CheckStop::default(), &[]).unwrap();
        eprintln!("note: {note}");
        assert!(passed && debug_apk(&app).is_some(), "{note}");
    }

    /// The check passes only when Gradle succeeds AND leaves a debug APK; a wrapper missing or a
    /// failed Gradle run fails it.
    #[cfg(unix)]
    #[test]
    fn the_build_passes_only_with_a_debug_apk() {
        let (app, sdk) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let sdk = sdk_at(sdk.path());
        let p = probe(false, &[("ANDROID_HOME", sdk.to_str().unwrap())], &["java"], vec![]);
        let run = || build(app.path(), "app/", &p, &CheckStop::default(), &[]).unwrap();
        let (passed, note) = run();
        assert!(!passed && note.contains("gradlew"), "{note}");
        let wrapper = |script: &str| std::fs::write(app.path().join("gradlew"), script).unwrap();
        wrapper("test -n \"$ANDROID_HOME\" && echo 'BUILD SUCCESSFUL'");
        let (passed, note) = run();
        assert!(!passed && note.contains(APK_DIR), "a successful Gradle run without an APK: {note}");
        wrapper(&format!("mkdir -p {APK_DIR} && touch {APK_DIR}/app-debug.apk"));
        let (passed, note) = run();
        assert!(passed && note.contains("app-debug.apk"), "{note}");
        wrapper("echo 'e: MainActivity.kt: Unresolved reference: Foo'; echo 'FAILURE: Build failed with an exception.'; exit 1");
        let (passed, note) = run();
        assert!(!passed && note.contains("Unresolved reference"), "the old APK doesn't save a failed build: {note}");
        wrapper("echo 'ERROR: JAVA_HOME is not set and no java command could be found'; exit 1");
        assert!(build(app.path(), "app/", &p, &CheckStop::default(), &[]).is_err_and(|e| missing_reason(&e)));
    }
}
