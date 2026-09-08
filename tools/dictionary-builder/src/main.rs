use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|command| command == "metadata") {
        if args.len() != 8 {
            eprintln!(
                "usage: dictionary-builder metadata <unidic-lex.csv> <pitch-sha256> <pitch-version> <jlpt-bank-directory> <jlpt-sha256> <jlpt-version> <output.sqlite>"
            );
            std::process::exit(2);
        }
        let pitch_source = PathBuf::from(&args[1]);
        let jlpt_directory = PathBuf::from(&args[4]);
        let output = PathBuf::from(&args[7]);
        match dictionary_builder::build_metadata(
            &pitch_source,
            &args[2],
            &args[3],
            &jlpt_directory,
            &args[5],
            &args[6],
            &output,
        ) {
            Ok((pitch_count, jlpt_count)) => println!(
                "Imported {pitch_count} pitch patterns and {jlpt_count} JLPT estimates into {}",
                output.display()
            ),
            Err(error) => {
                eprintln!("{}", error.message);
                if let Some(diagnostics) = error.diagnostics {
                    eprintln!("diagnostics: {diagnostics}");
                }
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() != 4 {
        eprintln!(
            "usage: dictionary-builder <JMdict.xml|jmdict-simplified.json> <sha256> <source-version> <output.sqlite>"
        );
        std::process::exit(2);
    }
    let source = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[3]);
    match dictionary_builder::build(&source, &args[1], &args[2], &output) {
        Ok(count) => println!("Imported {count} JMdict entries into {}", output.display()),
        Err(error) => {
            eprintln!("{}", error.message);
            if let Some(diagnostics) = error.diagnostics {
                eprintln!("diagnostics: {diagnostics}");
            }
            std::process::exit(1);
        }
    }
}
