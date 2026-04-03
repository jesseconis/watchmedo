use clap::{Command,Arg,ArgAction};

pub fn build_cli() -> Command {
    Command::new("compl")
        .about("Tests completions")
        .arg(Arg::new("file")
            .help("some input file"))
        .subcommand(Command::new("test")
            .about("tests things")
            .arg(Arg::new("case")
                .long("case")
                .action(ArgAction::Set)
                .help("the case to test")))
}

