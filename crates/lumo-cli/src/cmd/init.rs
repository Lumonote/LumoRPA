use clap::Args as ClapArgs;
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Project directory to create
    pub path: PathBuf,
    /// Project name
    #[arg(long)]
    pub name: Option<String>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let dir = &args.path;
    if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
        anyhow::bail!("directory {} is not empty", dir.display());
    }
    std::fs::create_dir_all(dir.join("flows"))?;
    std::fs::create_dir_all(dir.join("templates"))?;
    let name = args
        .name
        .clone()
        .or_else(|| dir.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-flow".to_string());

    let hello = include_str!("../../../../examples/hello-world.lumoflow.yaml");
    std::fs::write(dir.join("flows/hello.lumoflow.yaml"), hello.replace("hello-world", &name))?;

    std::fs::write(
        dir.join("README.md"),
        format!(
            "# {name}\n\nLumoRPA project.\n\nRun with:\n\n```bash\nlumo run flows/hello.lumoflow.yaml\n```\n"
        ),
    )?;
    println!("Initialized LumoRPA project at {}", dir.display());
    Ok(())
}
