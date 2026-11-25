mod archive;
mod classifier;
mod formatter;

use clap::Parser;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(version, about = "将 HAR 文件无损转换为多目录结构化 ZIP 归档")]
struct Args {
    /// 输入的 HAR 文件路径
    input: PathBuf,
    /// 输出的 ZIP 文件路径或目标目录（省略时默认在同目录生成同名 .zip）
    output: Option<PathBuf>,
}

fn resolve_output_path(input: &Path, output: Option<&Path>) -> PathBuf {
    let default_filename = input
        .file_stem()
        .filter(|stem| !stem.is_empty())
        .map_or_else(
            || PathBuf::from("output.zip"),
            |stem| {
                let mut name = stem.to_os_string();
                name.push(".zip");
                PathBuf::from(name)
            },
        );

    let Some(target) = output else {
        return input.with_file_name(default_filename);
    };

    let target_str = target.to_string_lossy();
    let ends_with_sep = target_str.ends_with('/') || target_str.ends_with('\\');
    if target.is_dir() || ends_with_sep {
        target.join(default_filename)
    } else if target.extension().is_none() {
        target.with_extension("zip")
    } else {
        target.to_path_buf()
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let output = resolve_output_path(&args.input, args.output.as_deref());
    match archive::convert_har_to_zip(&args.input, &output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误: {error}");
            ExitCode::FAILURE
        }
    }
}
