use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "obfuscation-cli",
    version,
    about = "Cocos JS 混淆命令行工具",
    propagate_version = true,
    disable_help_subcommand = true,
    disable_help_flag = true,
    disable_version_flag = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[arg(short = 'h', long = "help", action = ArgAction::Help, global = true, help = "显示帮助信息")]
    pub help: Option<bool>,

    #[arg(short = 'V', long = "version", action = ArgAction::Version, global = true, help = "显示版本信息")]
    pub version: Option<bool>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 执行完整混淆流程
    Run(RunArgs),
    /// 扫描资源目录并更新 whitelist.json 的 exclude 部分
    RefreshExclude(RefreshExcludeArgs),
    /// 从构建目录提取引擎关键字并更新 whitelist.json
    ExtractEngine(ExtractEngineArgs),
    /// 检查必需路径和文件是否存在
    Doctor(DoctorArgs),
}

#[derive(Debug, clap::Args, Clone)]
pub struct CommonPathArgs {
    #[arg(long, default_value = "assets", help = "资源目录路径")]
    pub assets_dir: PathBuf,

    #[arg(long, default_value = "python/obfuscation", help = "词典目录路径")]
    pub dict_dir: PathBuf,
}

#[derive(Debug, clap::Args, Clone)]
pub struct RunArgs {
    #[arg(
        long,
        conflicts_with = "input_file",
        help = "待处理 JS 目录（与 --input-file 二选一）"
    )]
    pub input_dir: Option<PathBuf>,

    #[arg(
        long,
        conflicts_with = "input_dir",
        help = "待处理单个 JS 文件（与 --input-dir 二选一）"
    )]
    pub input_file: Option<PathBuf>,

    #[arg(
        long,
        conflicts_with = "output_file",
        help = "输出目录（目录模式下必填）"
    )]
    pub output_dir: Option<PathBuf>,

    #[arg(
        long,
        conflicts_with = "output_dir",
        help = "输出单文件路径（单文件模式可选）"
    )]
    pub output_file: Option<PathBuf>,

    #[arg(long, help = "构建目录（刷新引擎词时需要）")]
    pub build_dir: Option<PathBuf>,

    #[command(flatten)]
    pub common: CommonPathArgs,

    #[arg(
        long,
        action = ArgAction::Set,
        default_value_t = true,
        help = "是否刷新白名单中的 exclude 部分（true/false）"
    )]
    pub refresh_exclude: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "刷新白名单中的引擎关键字")]
    pub refresh_engine: bool,

    #[arg(long, help = "并行线程数（默认使用 CPU 核数）")]
    pub threads: Option<usize>,

    #[arg(long, help = "随机种子（固定后可复现混淆结果）")]
    pub seed: Option<u64>,

    #[arg(long, action = ArgAction::SetTrue, help = "仅演练，不写出文件")]
    pub dry_run: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "遇到首个文件错误时立即失败")]
    pub fail_fast: bool,

    #[arg(
        long,
        action = ArgAction::Set,
        default_value_t = true,
        help = "兼容模式（预留，true/false）"
    )]
    pub compat_strict: bool,

    #[arg(long, default_value_t = 8, help = "混淆名最小长度")]
    pub name_min_len: usize,

    #[arg(long, default_value_t = 14, help = "混淆名最大长度")]
    pub name_max_len: usize,

    #[arg(long, default_value = "alnum-mixed", help = "混淆名字符集策略")]
    pub name_charset: NameCharset,

    #[arg(long, default_value = "letter", help = "混淆名首字符策略")]
    pub name_first_char: NameFirstChar,

    #[arg(long, help = "映射输出路径（默认: ./obfuscation_mapping.json）")]
    pub mapping_out: Option<PathBuf>,

    #[arg(long, action = ArgAction::SetTrue, help = "关闭映射文件输出")]
    pub no_mapping: bool,
}

#[derive(Debug, clap::Args, Clone)]
pub struct RefreshExcludeArgs {
    #[command(flatten)]
    pub common: CommonPathArgs,
}

#[derive(Debug, clap::Args, Clone)]
pub struct ExtractEngineArgs {
    #[arg(long, help = "构建目录路径")]
    pub build_dir: PathBuf,

    #[arg(long, default_value = "python/obfuscation", help = "词典目录路径")]
    pub dict_dir: PathBuf,
}

#[derive(Debug, clap::Args, Clone)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub common: CommonPathArgs,

    #[arg(long, help = "构建目录路径（可选）")]
    pub build_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum NameCharset {
    AlnumMixed,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum NameFirstChar {
    Letter,
}
