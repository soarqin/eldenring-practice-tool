use hudhook::tracing::info;
use pkg_version::*;
use semver::*;

const PRACTICE_TOOL_VERSION: Version = Version {
    major: pkg_version_major!(),
    minor: pkg_version_minor!(),
    patch: pkg_version_patch!(),
    pre: Prerelease::EMPTY,
    build: BuildMetadata::EMPTY,
};

const UPDATE_URL: &str =
    "https://api.github.com/repos/veeenu/eldenring-practice-tool/releases/latest";

pub enum Update {
    Available { url: String, notes: String },
    UpToDate,
    Error(String),
}

impl Update {
    pub fn check() -> Self {
        info!("正在检查更新...");
        #[derive(serde::Deserialize)]
        struct GithubRelease {
            tag_name: String,
            html_url: String,
            body: String,
        }

        let release = match ureq::get(UPDATE_URL).call() {
            Ok(release) => release,
            Err(e) => return Update::Error(e.to_string()),
        };

        let release = match release.into_json::<GithubRelease>() {
            Ok(release) => release,
            Err(e) => return Update::Error(e.to_string()),
        };

        let version = match Version::parse(&release.tag_name) {
            Ok(version) => version,
            Err(e) => return Update::Error(e.to_string()),
        };

        if version > PRACTICE_TOOL_VERSION {
            let notes = match release.body.find("## What's Changed") {
                Some(i) => release.body[..i].trim().to_string(),
                None => release.body,
            };
            let notes = format!(
                "发现有新版练习工具！\n\n最新版本:    \
                 {}\n已安装版本: {}\n\n更新内容:\n{}\n",
                version, PRACTICE_TOOL_VERSION, notes
            );

            let url = release.html_url;

            Update::Available { url, notes }
        } else {
            Update::UpToDate
        }
    }
}
