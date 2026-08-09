use std::path::PathBuf;

pub struct Config {
    pub port: u16,
    pub dir: PathBuf,
    pub dbfilename: String,
    pub save_seconds: Option<u64>,
}

impl Config {
    pub fn from_args() -> Result<Config, String> {
        let mut port = 6379;
        let mut dir = PathBuf::from(".");
        let mut dbfilename = "dump.rdb".to_string();
        let mut save_seconds: Option<u64> = None;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => port = parse_num(&mut args, "port")?,
                "--dir" => dir = PathBuf::from(next(&mut args, "dir")?),
                "--dbfilename" => dbfilename = next(&mut args, "dbfilename")?,
                "--save-seconds" => save_seconds = Some(parse_num(&mut args, "save-seconds")?),
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Config {
            port,
            dir,
            dbfilename,
            save_seconds,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.join(&self.dbfilename)
    }
}

fn next(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for --{name}"))
}

fn parse_num<T>(args: &mut impl Iterator<Item = String>, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let raw = next(args, name)?;
    raw.parse()
        .map_err(|_| format!("invalid value for --{name}: {raw}"))
}

fn print_help() {
    println!(
        "Usage: redis-server [OPTIONS]

Options:
  --port N            port to listen on (default: 6379)
  --dir PATH          directory for the snapshot file (default: .)
  --dbfilename NAME   snapshot file name (default: dump.rdb)
  --save-seconds N    autosave snapshot every N seconds when data changed
  --help              show this help"
    );
}
