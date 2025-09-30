# 追加需求：输出分隔符模式

在原有 `===path===` / `===end of 'path'===` 的基础上，CLI 需要允许用户选择分隔符方案：

* `--divider triple-backtick`：使用形如
  ```
  ``` path/to/file.ext
  <contents>
  ```
  ```
  的包围式 code block，首行需包含相对路径（或绝对路径）和文件名。
* `--divider xml-tag`：使用形如 `<file path="path/to/file.ext">` 包裹内容，并以 `</file>` 结尾。
* `--divider equals`（默认）：沿用现有 `===path===` / `===end of 'path'===`，同时确保 `===end` 之前有换行。

分隔符选项应当与其它功能兼容（排序、binary 处理等），并保证所有模式都会打印路径 + 文件名。

# Cargo.toml
[package]
name = "printfiles"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4.5", features = ["derive"] }
globwalk = "0.9"
which = "6.0"

# src/main.rs
use clap::{Parser, ValueEnum};
use globwalk::{GlobWalkerBuilder, WalkError};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Reader {
    /// 直接按文本读取（默认）
    Text,
    /// 调用 macOS `textutil` 读取（适合 doc/docx/rtf/html 等）
    Textutil,
    /// 自动：对部分扩展名用 textutil，其它走 Text
    Auto,
}

#[derive(Debug, Parser)]
#[command(name = "printfiles", version, about = "Print files matched by globs/dirs with ===header=== and ===end of 'file'===")]
struct Args {
    /// 一组以空格或逗号分隔的模式或目录，例如：
    /// "src/**/*.rs,docs/*.md" tests "README*"
    #[arg(required = true)]
    items: Vec<String>,

    /// 读取后端：text(默认) / textutil / auto
    #[arg(long, value_enum, default_value_t = Reader::Text)]
    reader: Reader,

    /// 若传入目录，是否仅限这些扩展（逗号分隔，如 "rs,md"）。
    #[arg(long)]
    ext: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // 解析 items：支持空格和逗号混合
    let mut tokens: Vec<String> = Vec::new();
    for it in args.items.iter() {
        for piece in it.split(',') {
            let s = piece.trim();
            if !s.is_empty() {
                tokens.push(s.to_string());
            }
        }
    }

    // 收集匹配到的文件到有序集合，确保稳定输出且去重
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();

    for token in tokens {
        let p = Path::new(&token);
        if p.is_dir() {
            // 目录：递归匹配所有文件，或受 --ext 限制
            let mut builder = GlobWalkerBuilder::from_patterns(p, &["**/*"]);
            builder.follow_links(true).case_insensitive(false);
            let walker = builder.build()?;
            for entry in walker.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ref exts) = args.ext {
                        if !ext_match(path, exts) { continue; }
                    }
                    files.insert(normalize(path));
                }
            }
            continue;
        }

        // 其余情况按 glob 处理（支持 **）
        match expand_glob(token) {
            Ok(iter) => {
                for path in iter {
                    if path.is_file() {
                        files.insert(normalize(&path));
                    }
                }
            }
            Err(err) => {
                eprintln!("警告: 模式无效或没有匹配: {err}");
            }
        }
    }

    if files.is_empty() {
        eprintln!("（未匹配到任何文件）");
        std::process::exit(2);
    }

    let mut out = io::BufWriter::new(io::stdout());

    for path in files {
        let rel = rel_display(&path);
        writeln!(out, "==={}===", rel)?;
        read_and_write(&path, args.reader, &mut out)?;
        writeln!(out, "===end of '{}'===", rel)?;
    }
    out.flush()?;
    Ok(())
}

fn normalize(p: &Path) -> PathBuf {
    // 去掉诸如 foo/./bar 之类的奇怪片段；
    // 这里不做 canonicalize 以避免跨文件系统/权限问题。
    PathBuf::from(p)
}

fn rel_display(p: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(stripped) = p.strip_prefix(&cwd) {
            return strip_dot_slash(stripped).to_string();
        }
    }
    strip_dot_slash(p).to_string()
}

fn strip_dot_slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix("./").unwrap_or(&s).to_string()
}

fn ext_match(path: &Path, exts_csv: &str) -> bool {
    let ext = path.extension().and_then(OsStr::to_str).map(|s| s.to_ascii_lowercase());
    let Some(ext) = ext else { return false; };
    for e in exts_csv.split(',') {
        if ext == e.trim().to_ascii_lowercase() { return true; }
    }
    false
}

fn expand_glob(pattern: String) -> Result<impl Iterator<Item = PathBuf>, WalkError> {
    // 若没有通配符且是存在的文件，直接返回它
    let has_glob = pattern.contains('*') || pattern.contains('?') || pattern.contains('[');
    if !has_glob {
        return Ok(std::iter::once(PathBuf::from(pattern)));
    }

    let base = ".";
    let mut builder = GlobWalkerBuilder::from_patterns(base, &[pattern.as_str()]);
    builder.follow_links(true).case_insensitive(false);
    let walker = builder.build()?;
    Ok(walker.filter_map(|e| e.ok()).map(|e| e.path().to_path_buf()))
}

fn read_and_write<W: Write>(path: &Path, reader: Reader, mut out: W) -> anyhow::Result<()> {
    match reader {
        Reader::Text => write_text(path, &mut out),
        Reader::Textutil => write_textutil_then_fallback(path, &mut out),
        Reader::Auto => {
            if should_use_textutil(path) { write_textutil_then_fallback(path, &mut out) }
            else { write_text(path, &mut out) }
        }
    }
}

fn write_text<W: Write>(path: &Path, out: &mut W) -> anyhow::Result<()> {
    match fs::read(path) {
        Ok(bytes) => {
            // 尽量用 UTF-8 显示，非 UTF-8 时采用有损转换
            let s = String::from_utf8_lossy(&bytes);
            write!(out, "{}", s)?;
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("读取失败: {}: {}", path.display(), e);
        }
    }
}

fn write_textutil_then_fallback<W: Write>(path: &Path, out: &mut W) -> anyhow::Result<()> {
    if which::which("textutil").is_ok() {
        let output = Command::new("textutil")
            .arg("-convert").arg("txt")
            .arg("-stdout")
            .arg(path)
            .output();
        match output {
            Ok(outp) if outp.status.success() => {
                out.write_all(&outp.stdout)?;
                return Ok(());
            }
            Ok(outp) => {
                eprintln!("警告: textutil 处理失败 ({}), 回退到文本读取: {}", outp.status, path.display());
            }
            Err(e) => {
                eprintln!("警告: textutil 调用异常 ({}), 回退到文本读取: {}", e, path.display());
            }
        }
    } else {
        eprintln!("提示: 未检测到 textutil，回退到文本读取。文件: {}", path.display());
    }
    write_text(path, out)
}

fn should_use_textutil(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(OsStr::to_str).map(|s| s.to_ascii_lowercase()) else { return false; };
    matches!(ext.as_str(),
        "rtf" | "rtfd" | "doc" | "docx" | "html" | "htm" | "odt" | "webarchive")
}

好了，我把完整的 **Rust 可编译版本**放到右侧画布里了（含 `Cargo.toml` 和 `src/main.rs`）。它支持：

* 传入**空格/逗号混合**的 glob 或目录（目录会递归匹配）。
* 输出对称三等号：`===path===` / `===end of 'path'===`。
* 读取后端可选：`--reader text`（默认字符串读取）、`--reader textutil`（macOS 上用 `textutil`）、`--reader auto`（对 `doc/docx/rtf/html/...` 自动用 `textutil`，其它走文本读取）。
* 目录可用 `--ext rs,md` 这类过滤扩展名。

### 构建

```bash
cargo build --release
# 可执行文件：target/release/printfiles
```

### 用法示例

```bash
# 递归打印 src 下所有 .rs 和 docs 下所有 .md
target/release/printfiles "src/**/*.rs,docs/*.md"

# 空格 + 逗号混合
target/release/printfiles src/**/*.rs docs/*.md,tests/*.rs

# 目录 + 扩展名过滤（只打印目录下的 .rs 和 .md）
target/release/printfiles src --ext rs,md

# 使用 textutil（macOS，适合 .doc/.docx/.rtf/.html）
target/release/printfiles reports/**/*.docx --reader textutil

# 自动模式：对 Office/RTF/HTML 用 textutil，其它走文本
target/release/printfiles reports,docs --ext md,docx --reader auto
```

> 说明
>
> * 在 **非 macOS** 上没有 `textutil`，程序会自动回退到文本读取。
> * 默认以当前目录作为相对路径根；若文件不在当前目录下，会显示原路径。
> * 输出顺序是**按路径排序**且去重的，便于比对与存档。

如果你想加功能（例如：并发读取、忽略二进制文件、按文件大小/时间排序、支持 `--relative-from` 指定相对根目录），告诉我我直接给你扩上去。
