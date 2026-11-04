fn main() {
    assert_eq!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("macos"),
        "AnywhereHost requires macOS"
    );
    let project = "../AnywhereHost.xcodeproj/project.pbxproj";
    println!("cargo:rerun-if-changed={project}");
    let source = std::fs::read_to_string(project).expect("Host version configuration");
    let builds: std::collections::BTreeSet<u64> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("CURRENT_PROJECT_VERSION = "))
        .map(|value| {
            value
                .trim_end_matches(';')
                .parse()
                .expect("numeric build number")
        })
        .collect();
    assert_eq!(builds.len(), 1, "Host build numbers must match");
    println!("cargo:rustc-env=ANYWHERE_BUILD={}", builds.first().unwrap());
}
