use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rayon::ThreadPoolBuilder;

pub mod cli;
pub mod pipeline;

use cli::{Cli, Command, DoctorArgs, ExtractEngineArgs, RefreshExcludeArgs, RunArgs};
use pipeline::obfuscate::{RunConfig, run as run_obfuscation};
use pipeline::whitelist::{
    ensure_whitelist, refresh_whitelist_engine, refresh_whitelist_exclude, whitelist_path,
};

pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_command(args),
        Command::RefreshExclude(args) => refresh_exclude_command(args),
        Command::ExtractEngine(args) => extract_engine_command(args),
        Command::Doctor(args) => doctor_command(args),
    }
}

fn run_command(args: RunArgs) -> Result<()> {
    validate_run_args(&args)?;

    // 统一配置 Rayon 全局线程池，保证后续并行提取与替换使用同一线程数。
    let threads = args.threads.unwrap_or_else(num_cpus::get);
    ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .ok();

    let dict_dir = &args.common.dict_dir;
    let assets_dir = &args.common.assets_dir;

    let whitelist = ensure_whitelist(
        dict_dir,
        assets_dir,
        args.build_dir.as_deref(),
        args.refresh_engine,
        args.refresh_exclude,
    )?;

    // 先建立全局映射，再并行处理文件，确保同名符号在所有文件中替换一致。
    let summary = run_obfuscation(
        RunConfig {
            input_dir: args.input_dir,
            input_file: args.input_file,
            output_dir: args.output_dir,
            output_file: args.output_file,
            dict_dir: dict_dir.clone(),
            dry_run: args.dry_run,
            fail_fast: args.fail_fast,
            seed: args.seed,
            min_len: args.name_min_len,
            max_len: args.name_max_len,
            no_mapping: args.no_mapping,
            mapping_out: args.mapping_out,
        },
        &whitelist.engine_keywords,
        &whitelist.merged_keywords,
    )?;

    println!("处理文件数: {}", summary.file_count);
    println!("替换符号总数: {}", summary.replaced_word_count);
    if let Some(path) = summary.mapping_path {
        println!("映射文件: {}", path.display());
    }
    if !summary.output_files.is_empty() {
        println!("首个输出文件: {}", summary.output_files[0].display());
    }
    println!("白名单文件: {}", whitelist_path(dict_dir).display());

    Ok(())
}

fn refresh_exclude_command(args: RefreshExcludeArgs) -> Result<()> {
    refresh_whitelist_exclude(&args.common.dict_dir, &args.common.assets_dir)?;
    println!(
        "已更新: {}",
        whitelist_path(&args.common.dict_dir).display()
    );
    Ok(())
}

fn extract_engine_command(args: ExtractEngineArgs) -> Result<()> {
    refresh_whitelist_engine(&args.dict_dir, &args.build_dir)?;
    println!("已更新: {}", whitelist_path(&args.dict_dir).display());
    Ok(())
}

fn doctor_command(args: DoctorArgs) -> Result<()> {
    check_path_exists("assets-dir", &args.common.assets_dir)?;
    check_path_exists("dict-dir", &args.common.dict_dir)?;
    check_path_exists(
        "js_keywords.txt",
        &args.common.dict_dir.join("js_keywords.txt"),
    )?;
    check_path_exists(
        "lexicon_origin.txt",
        &args.common.dict_dir.join("lexicon_origin.txt"),
    )?;
    check_path_exists(
        "lexicon_origin1.txt",
        &args.common.dict_dir.join("lexicon_origin1.txt"),
    )?;
    if let Some(build) = args.build_dir.as_deref() {
        check_path_exists("build-dir", build)?;
    }
    println!("检查通过");
    Ok(())
}

fn check_path_exists(label: &str, path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("{} 不存在: {}", label, path.display());
    }
    Ok(())
}

fn validate_run_args(args: &RunArgs) -> Result<()> {
    if args.input_dir.is_none() && args.input_file.is_none() {
        bail!("必须提供 --input-dir 或 --input-file 其中之一");
    }
    if args.input_dir.is_some() && args.output_dir.is_none() {
        bail!("使用 --input-dir 时必须提供 --output-dir");
    }
    if args.input_file.is_some() && args.output_dir.is_some() {
        bail!("--input-file 模式下不能同时使用 --output-dir");
    }
    if args.name_min_len == 0 || args.name_max_len < args.name_min_len {
        bail!("混淆名长度范围非法");
    }
    if args.refresh_engine && args.build_dir.is_none() {
        bail!("使用 --refresh-engine 时必须提供 --build-dir");
    }
    if !args.common.dict_dir.exists() {
        return Err(anyhow::anyhow!(
            "dict-dir 不存在: {}",
            args.common.dict_dir.display()
        ))
        .context("--dict-dir 参数无效");
    }
    Ok(())
}
