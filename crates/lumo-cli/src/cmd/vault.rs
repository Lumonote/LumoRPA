//! `lumo vault` subcommand — age-encrypted secret store (P1-3).

use clap::{Args as ClapArgs, Subcommand};
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL, Table};
use lumo_storage::{vault, Repo, Vault, VaultIdentity};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::vault_identity_path;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    sub: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// Generate the age identity file (your master private key)
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Add or update a secret field. Value is read from a hidden prompt, or
    /// `--stdin`. NEVER pass the value on the command line.
    Add {
        name: String,
        #[arg(long)]
        key: Option<String>,
        /// Read the value from stdin instead of a hidden prompt
        #[arg(long)]
        stdin: bool,
    },
    /// Show a stored item — masked unless `--reveal`
    Get {
        name: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        reveal: bool,
    },
    /// List stored item names + field keys (never reveals values)
    List,
    /// Remove a whole item, or a single field with `--key`
    Rm {
        name: String,
        #[arg(long)]
        key: Option<String>,
    },
    /// Print the identity file path and DB path
    Path,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let id_path = vault_identity_path(&home);
    match args.sub {
        Sub::Path => {
            println!("identity: {}", id_path.display());
            println!("db:       {}", home.join("lumo.db").display());
            Ok(())
        }

        Sub::Init { force } => {
            if id_path.exists() && !force {
                anyhow::bail!(
                    "{} already exists. Use --force to overwrite (DESTROYS access to existing secrets).",
                    id_path.display()
                );
            }
            let identity = VaultIdentity::generate();
            identity.save(&id_path)?;
            println!(
                "{} wrote identity to {}",
                "✓".green().bold(),
                id_path.display()
            );
            println!("  public key: {}", identity.public_string());
            println!(
                "  {} add `{}` to .gitignore — it is your master key.",
                "!".yellow().bold(),
                id_path.display()
            );
            Ok(())
        }

        Sub::Add { name, key, stdin } => {
            let identity = load_identity(&id_path)?;
            let repo = open_repo(&home)?;
            let value = read_secret_value(stdin)?;
            let vault = Vault::new(&repo, &identity);
            let mut fields = vault.get(&name)?.unwrap_or_default();
            upsert_field(&mut fields, &key.unwrap_or_default(), value);
            vault.put(&name, &fields)?;
            println!("{} stored secret `{}`", "✓".green().bold(), name);
            Ok(())
        }

        Sub::Get { name, key, reveal } => {
            let identity = load_identity(&id_path)?;
            let repo = open_repo(&home)?;
            let fields = Vault::new(&repo, &identity)
                .get(&name)?
                .ok_or_else(|| anyhow::anyhow!("no vault item `{name}`"))?;
            match key {
                Some(k) => {
                    let v = fields
                        .get(&k)
                        .ok_or_else(|| anyhow::anyhow!("item `{name}` has no key `{k}`"))?;
                    println!("{}", if reveal { v.clone() } else { mask() });
                }
                None => {
                    for (k, v) in &fields {
                        let shown = if reveal { v.clone() } else { mask() };
                        let label = if k.is_empty() { "(scalar)" } else { k.as_str() };
                        println!("{label} = {shown}");
                    }
                }
            }
            Ok(())
        }

        Sub::List => {
            let repo = open_repo(&home)?;
            let items = vault::list(&repo)?;
            if items.is_empty() {
                println!("(vault is empty — add one with `lumo vault add <name> --key <k>`)");
                return Ok(());
            }
            let mut t = Table::new();
            t.load_preset(UTF8_FULL)
                .set_header(vec!["name", "keys", "updated_at"]);
            for it in items {
                let updated = chrono::DateTime::from_timestamp_millis(it.updated_at)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| it.updated_at.to_string());
                t.add_row(vec![it.name, it.keys.join(", "), updated]);
            }
            println!("{t}");
            Ok(())
        }

        Sub::Rm { name, key } => {
            let identity = load_identity(&id_path)?;
            let repo = open_repo(&home)?;
            let vault = Vault::new(&repo, &identity);
            match key {
                None => {
                    vault.delete(&name)?;
                    println!("{} removed `{}`", "✓".green().bold(), name);
                }
                Some(k) => {
                    let mut fields = vault
                        .get(&name)?
                        .ok_or_else(|| anyhow::anyhow!("no vault item `{name}`"))?;
                    let now_empty = remove_field(&mut fields, &k);
                    if now_empty {
                        vault.delete(&name)?;
                    } else {
                        vault.put(&name, &fields)?;
                    }
                    println!("{} removed `{}.{}`", "✓".green().bold(), name, k);
                }
            }
            Ok(())
        }
    }
}

fn open_repo(home: &Path) -> anyhow::Result<Repo> {
    std::fs::create_dir_all(home)?;
    Ok(Repo::open(home.join("lumo.db"))?)
}

fn load_identity(id_path: &Path) -> anyhow::Result<VaultIdentity> {
    if !id_path.exists() {
        anyhow::bail!(
            "vault identity not found at {}; run `lumo vault init` first",
            id_path.display()
        );
    }
    Ok(VaultIdentity::load(id_path)?)
}

fn read_secret_value(stdin: bool) -> anyhow::Result<String> {
    if stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Ok(buf.trim_end_matches(['\n', '\r']).to_string())
    } else {
        Ok(rpassword::prompt_password("secret value: ")?)
    }
}

/// Fixed-width mask — never leaks the value or its length.
fn mask() -> String {
    "********".to_string()
}

/// Insert or overwrite a single field. Pure (testable) core of `add`.
fn upsert_field(fields: &mut BTreeMap<String, String>, key: &str, value: String) {
    fields.insert(key.to_string(), value);
}

/// Remove a field; returns true when the item is now empty (caller deletes it).
fn remove_field(fields: &mut BTreeMap<String, String>, key: &str) -> bool {
    fields.remove(key);
    fields.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn mask_hides_value_and_length() {
        assert_eq!(mask(), "********");
        assert_eq!(mask(), mask(), "mask is constant — never leaks length");
    }

    #[test]
    fn upsert_inserts_then_overwrites() {
        let mut f = BTreeMap::new();
        upsert_field(&mut f, "user", "a".to_string());
        assert_eq!(f.get("user").map(String::as_str), Some("a"));
        upsert_field(&mut f, "user", "b".to_string());
        assert_eq!(f.get("user").map(String::as_str), Some("b"));
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn remove_reports_emptiness() {
        let mut f = BTreeMap::new();
        upsert_field(&mut f, "user", "a".to_string());
        upsert_field(&mut f, "pass", "b".to_string());
        assert!(!remove_field(&mut f, "user"), "pass still present");
        assert!(remove_field(&mut f, "pass"), "now empty");
    }
}
