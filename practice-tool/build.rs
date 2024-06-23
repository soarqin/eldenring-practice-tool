use std::collections::BTreeSet;
use std::path::Path;

// UI characters from the Chinese core revision pinned in Cargo.lock.
const CORE_UI_CHARACTERS: &str = concat!(
    "…。「」一下不与丢个习于件传位例保信修值停入关具",
    "内写出分列创删别到制刷前加动助包升单卢原发取可台",
    "合名否含启品器回在型处外失如存家容导将少属工已布",
    "帧帮幅应度建开弃当录径微志快态性恩息意戏成手找择",
    "持指按换捷控提支改放效数整文新无日时明是显暂更替",
    "最有期未本条析柄标档模止正没法活浏消添清游激父版",
    "物状玩理生用画界留的盖盘目知示禁离秒移程空符筛签",
    "管类级线练组编缺置能致色菜藏表覆览角解触计设识试",
    "说请读调距路轮载辑输近退送选速配重错键长闭间降除",
    "隐隔需面项颜饰！（），：？",
);

fn collect_ui_characters(path: &Path, characters: &mut BTreeSet<char>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("Read UI source directory") {
            collect_ui_characters(&entry.expect("Read UI source entry").path(), characters);
        }
    } else if matches!(path.extension().and_then(|ext| ext.to_str()), Some("rs" | "toml")) {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = std::fs::read_to_string(path).expect("Read UI source text");
        characters.extend(source.chars().filter(|c| !c.is_ascii() && !c.is_control()));
    }
}

fn main() {
    embed_resource::compile("./src/resources.rc", embed_resource::NONE);
    println!("cargo:rustc-cdylib-link-arg=/DEF:lib/no-logo/exports.def");

    // Bundle only UI characters, while keeping the runtime font fallback.
    let mut characters = CORE_UI_CHARACTERS.chars().collect::<BTreeSet<_>>();
    for path in ["src", "../jdsd_er_practice_tool.toml", "../lib/libeldenring/src/catalog"] {
        println!("cargo:rerun-if-changed={path}");
        collect_ui_characters(Path::new(path), &mut characters);
    }
    let output = Path::new(&std::env::var_os("OUT_DIR").expect("Build output directory"))
        .join("ui-glyphs.txt");
    std::fs::write(output, characters.into_iter().collect::<String>())
        .expect("Write UI characters");
}
