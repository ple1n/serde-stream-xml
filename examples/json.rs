use anyhow::Result;
use std::fs::File;
use std::io::Read;

const PATH: &str = "./beh.xml";

fn main() -> Result<()> {
    let mut rd = File::open(PATH)?;
    let mut p = xml::Parser::new();
    let mut e = xml::ElementBuilder::new();
    let mut found = 0;
    let mut deco = String::new();
    rd.read_to_string(&mut deco)?;
    dbg!(&deco);
    p.feed_str(&deco[..20]);
    for ev in &mut p{
        let (ev, pos) = ev?;
        let x = e.handle_event(ev);
        if let Some(el) = x {
            let el = el?;
            let serded = serde_json::to_string_pretty(&el)?;
            println!("{}", serded);
            found += 1;

            if found > 10 {
                break;
            }
        }
    }
    println!("second");
    p.feed_str(&deco[20..]);
    for ev in &mut p{
        let (ev, pos) = ev?;
        let x = e.handle_event(ev);
        if let Some(el) = x {
            let el = el?;
            let serded = serde_json::to_string_pretty(&el)?;
            println!("{}", serded);
            found += 1;

            if found > 10 {
                break;
            }
        }
    }
    Ok(())
}
