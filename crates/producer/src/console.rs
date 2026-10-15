//! The probe console: part of every run while the debug menu is on (the launcher leaves
//! [`monaka_contract::manifest::DEBUG_MENU_MARKER`] in the run folder; [`open`] at the run's start,
//! [`close`] first thing at its stop), it answers text commands from a tool (`monaka_play
//! console`, the Console panel) through the run's console channel, on a thread of its own. What it does is
//! research on the running game with no rebuild. Watching: probes at any function
//! (`monaka_hook::live`: calls, threads, arguments, returns, callers, the frame order, the fields
//! an action changes), hardware write watches (`monaka_hook::watch`), memory and class names read.
//! Finding: what code runs only while the player does something, and what code is handed an
//! object (`monaka_hook::coverage`); the code using a text, and classes by name with their methods
//! (`monaka_core::scan`, from the game's own image); a value inside an object. `help` lists the
//! commands.
//!
//! Addresses are written `exe+0x1234` (the game's executable), `name.dll+0x1234`, or `0x7ff6...`.
//! Every probe, breakpoint and watch is taken out when the run stops, before anything else stops.

use monaka_channel::ChannelName;
use monaka_channel::console::ConsoleChannel;
use monaka_core::sync::Locked;
use monaka_hook::module::Module;
use std::fmt::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(test)]
mod tests;

static CHANNEL: Mutex<Option<ConsoleChannel>> = Mutex::new(None);
/// Questions are looked for this often.
const EVERY: Duration = Duration::from_millis(50);
static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static CLOSING: AtomicBool = AtomicBool::new(false);

/// Opens the console on the run's channel and starts answering, on a thread of its own.
pub fn open(channel: &ChannelName) {
    let opened = match ConsoleChannel::open(channel) {
        Ok(opened) => opened,
        Err(e) => {
            crate::log!("console: not opened ({e})");
            return;
        }
    };
    *CHANNEL.locked() = Some(opened);
    CLOSING.store(false, Relaxed);
    let thread = std::thread::Builder::new().name("monaka console".into()).spawn(|| {
        while !CLOSING.load(Relaxed) {
            tick(EVERY.as_secs_f64());
            std::thread::sleep(EVERY);
        }
    });
    match thread {
        Ok(thread) => {
            *THREAD.locked() = Some(thread);
            crate::log!("console: open (monaka_play console \"help\")");
        }
        Err(e) => crate::log!("console: no thread ({e})"),
    }
}

/// Stops answering and takes every probe, breakpoint and watch out (at the run's stop, before
/// anything else stops).
pub fn close() {
    let thread = THREAD.locked().take();
    let Some(thread) = thread else { return };
    CLOSING.store(true, Relaxed);
    let _ = thread.join();
    stop();
    *CHANNEL.locked() = None;
}

/// Every probe and watch out.
fn stop() {
    #[cfg(target_arch = "x86_64")]
    {
        monaka_hook::live::remove_all();
        monaka_hook::coverage::disarm();
    }
    monaka_hook::watch::disarm();
}

/// The status is written again this often (timer ticks).
const STATUS_EVERY: u32 = 10;
static TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// On the run's timer: the question waiting, answered and logged; the status now and then.
fn tick(_: f64) {
    let pending = CHANNEL.locked().as_mut().and_then(ConsoleChannel::pending);
    if let Some((number, command)) = pending {
        let answer = run(&command);
        if let Some(channel) = CHANNEL.locked().as_mut() {
            channel.answer(number, &answer);
            channel.post(&command, &answer);
        }
    }
    if TICKS.fetch_add(1, Relaxed).is_multiple_of(STATUS_EVERY) {
        let text = status();
        if let Some(channel) = CHANNEL.locked().as_mut() {
            channel.set_status(&text);
        }
    }
}

/// What the console has in: its probes, its breakpoints, its watches.
pub fn status() -> String {
    let mut out = String::new();
    #[cfg(target_arch = "x86_64")]
    {
        let probes: Vec<_> = monaka_hook::live::probes().into_iter().filter(|p| p.enabled).collect();
        let _ = writeln!(out, "{} probe{} in", probes.len(), if probes.len() == 1 { "" } else { "s" });
        for p in &probes {
            let _ = writeln!(out, "  {} {}  {:.1} a frame{}", p.id, p.name, p.count as f64 / p.frames.max(1) as f64, if p.threads.len() > 1 { format!(", {} threads", p.threads.len()) } else { String::new() });
        }
        use monaka_hook::coverage;
        if coverage::armed() {
            let _ = writeln!(out, "tripwires in: {} functions ran, {} since Idle done", coverage::hits().len(), coverage::since().len());
        }
        if let Some(search) = search_status() {
            let _ = writeln!(out, "{search}");
        }
    }
    let hits = monaka_hook::watch::hits().len();
    if hits > 0 {
        let _ = writeln!(out, "watches: {hits} hits");
    }
    out
}

const HELP: &str = "\
help                          this list
where <addr>                  the module and offset of an address
modules [text]                loaded modules (those whose name holds text)
read <addr> [bytes]           memory as hex, with each 4 bytes as an integer and a float
class <addr>                  the C++ class of the object at addr (from its RTTI)
hook <addr> [name] [ret] [snap=<reg>[+off]:<bytes>] [only=<reg>:<value>] [force]
                              a probe at a function's start (force: anywhere);
                              ret records each call's return (rax, xmm0, how long it took);
                              snap copies bytes at reg (rcx/this, rdx, r8, r9) + off at each call;
                              only keeps the calls whose reg holds value (one object of many)
unhook <id|all>               takes probes out
list                          every probe: calls, calls a frame, threads, callers
calls <id> [n]                the probe's last n calls: thread, frame, arguments, caller
timeline [frames]             the calls of the last frames in order, with each present
mark <id>                     keeps the probe's latest snapshot to compare with
diff <id>                     the 4-byte fields that changed since the mark
snap <id>                     the probe's latest snapshot
watch <addr>... [reads]       hardware watches (up to 4) on writes (or reads too)
hits                          what the watches caught: the instruction, its thread, code on its stack
unwatch                       takes the watches off

Finding:
strings <text> [module]       texts holding text (any case) and the functions using them
classes <text> [module]       C++ classes whose name holds text: their method tables and methods
methods <addr>                the methods of the method table at addr
find <addr> <bytes> <value>   where value is in the bytes at addr (1.5: a float; 0x10 or 16: an integer)
cover start [budget] [module] a tripwire on every function: a search for an action's code begins
cover action [n]              you did the action: keeps the candidates that ran
cover inaction [n]            you played without it: keeps the candidates that did not run
                              (each round watches what is left again; repeat both to narrow)
cover undo                    back to the candidates before the last Action or Inaction
cover candidates [n]          the candidates left
cover value <value|off>       hits also check rcx, rdx, r8, r9 for value (cover start 32 checks more calls)
cover matches                 the functions handed the value, and in which register
cover hits [n]                everything that ran since cover start
cover stop                    every breakpoint out";

/// Answers one command.
pub fn run(command: &str) -> String {
    let words: Vec<&str> = command.split_whitespace().collect();
    let Some((&verb, rest)) = words.split_first() else { return HELP.into() };
    let result = match verb.to_ascii_lowercase().as_str() {
        "help" | "?" => Ok(HELP.into()),
        "where" => one_address(rest).map(Module::describe),
        "modules" => Ok(modules(rest.first().copied())),
        "read" => one_address(rest).map(|a| read(a, rest.get(1).and_then(|n| number(n)).unwrap_or(64) as usize)),
        "class" => one_address(rest).map(|a| monaka_hook::probe::class_name(a).unwrap_or_else(|| "no RTTI there".into())),
        "watch" => watch(rest),
        "hits" => Ok(hits()),
        "unwatch" => Ok(format!("watches off on {} threads", monaka_hook::watch::disarm())),
        "strings" => strings(rest),
        "classes" => classes(rest),
        "methods" => one_address(rest).and_then(methods),
        "find" => find(rest),
        _ => probes(verb, rest),
    };
    result.unwrap_or_else(|e| format!("error: {e}"))
}

#[cfg(target_arch = "x86_64")]
fn probes(verb: &str, rest: &[&str]) -> Result<String, String> {
    use monaka_hook::live;
    let id = || rest.first().and_then(|w| w.parse::<usize>().ok()).ok_or_else(|| "which probe? (its id, from list)".to_owned());
    match verb {
        "hook" => {
            let address = one_address(rest)?;
            let mut name = rest.first().copied().unwrap_or("").to_owned();
            let (mut snapshot, mut only, mut returns) = (None, None, false);
            for word in &rest[1..] {
                if *word == "ret" {
                    returns = true;
                } else if *word == "force" {
                } else if let Some(spec) = word.strip_prefix("snap=") {
                    snapshot = Some(parse_snapshot(spec)?);
                } else if let Some(spec) = word.strip_prefix("only=") {
                    let (register, value) = spec.split_once(':').ok_or("only=reg:value")?;
                    let register = live::Register::parse(register).ok_or("only: the register is rcx, rdx, r8 or r9")?;
                    only = Some((register, number(value).ok_or("only: the value")?));
                } else {
                    name = (*word).to_owned();
                }
            }
            if !rest.contains(&"force") {
                function_start(address)?;
            }
            monaka_hook::coverage::release_at(address);
            let id = live::add(live::Spec { address, name: name.clone(), snapshot, only, returns })?;
            Ok(format!("probe {id}: {name} at {}", Module::describe(address)))
        }
        "unhook" if rest.first() == Some(&"all") => {
            live::remove_all();
            Ok("every probe out".into())
        }
        "unhook" => live::remove(id()?).map(|()| "out".into()),
        "list" => Ok(list()),
        "calls" => Ok(calls(id()?, rest.get(1).and_then(|n| n.parse().ok()).unwrap_or(8))),
        "timeline" => Ok(timeline(rest.first().and_then(|n| n.parse().ok()).unwrap_or(2))),
        "mark" => live::mark(id()?).map(|n| format!("marked ({n} bytes)")),
        "diff" => {
            let changed = live::diff(id()?)?;
            let mut out = format!("{} fields changed\n", changed.len());
            for (offset, before, after) in changed {
                let _ = writeln!(out, "  +{offset:#06x}  {:08x} -> {:08x}   {} -> {}", u32::from_le_bytes(before), u32::from_le_bytes(after), f32::from_le_bytes(before), f32::from_le_bytes(after));
            }
            Ok(out)
        }
        "snap" => live::snapshot(id()?).map(|bytes| dump(0, &bytes)).ok_or_else(|| "no snapshot yet".into()),
        "cover" => cover(rest),
        _ => Err(format!("unknown command {verb}; help lists them")),
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn probes(verb: &str, _: &[&str]) -> Result<String, String> {
    Err(format!("{verb}: probes are for 64-bit games only"))
}

#[cfg(target_arch = "x86_64")]
fn list() -> String {
    let probes = monaka_hook::live::probes();
    if probes.is_empty() {
        return "no probes".into();
    }
    let mut out = String::new();
    for p in probes {
        let rate = p.count as f64 / p.frames.max(1) as f64;
        let threads: Vec<String> = p.threads.iter().map(|(t, n)| format!("{t} x{n}")).collect();
        let _ = writeln!(out, "{} {} at {}{}: {} calls, {rate:.2} a frame; threads {}", p.id, p.name, Module::describe(p.address), if p.enabled { "" } else { " (out)" }, p.count, threads.join(", "));
        for (caller, n) in p.callers.iter().take(6) {
            let _ = writeln!(out, "    from {} x{n}", Module::describe(*caller));
        }
    }
    out
}

#[cfg(target_arch = "x86_64")]
fn calls(id: usize, most: usize) -> String {
    let mut out = String::new();
    for c in monaka_hook::live::calls(id, most) {
        let _ = writeln!(
            out,
            "frame {} thread {} from {}\n    rcx {:#x}  rdx {:#x}  r8 {:#x}  r9 {:#x}\n    xmm0-3 {:?}  stack {:#x} {:#x}{}",
            c.frame,
            c.thread,
            Module::describe(c.caller),
            c.args[0],
            c.args[1],
            c.args[2],
            c.args[3],
            c.floats,
            c.stack[0],
            c.stack[1],
            c.returned.map_or_else(String::new, |r| format!("\n    returned rax {:#x} ({}), xmm0 {} after {} us", r.rax, r.rax as i64, r.xmm0, r.took))
        );
    }
    if out.is_empty() { "no calls yet".into() } else { out }
}

#[cfg(target_arch = "x86_64")]
fn timeline(frames: u64) -> String {
    use monaka_hook::live::Event;
    let names: std::collections::HashMap<usize, String> = monaka_hook::live::probes().into_iter().map(|p| (p.id, p.name)).collect();
    let events = monaka_hook::live::timeline(frames);
    let start = events.first().map_or(0, |e| match e {
        Event::Call(c) => c.time,
        Event::Present { time, .. } => *time,
    });
    let mut out = String::new();
    for event in events {
        match event {
            Event::Call(c) => {
                let _ = writeln!(out, "  {:>8} us  thread {:>6}  {}  from {}", c.time - start, c.thread, names.get(&c.probe).map_or("?", String::as_str), Module::describe(c.caller));
            }
            Event::Present { frame, time, thread } => {
                let _ = writeln!(out, "-- present, frame {frame} ends ({} us, thread {thread})", time - start);
            }
        }
    }
    if out.is_empty() { "nothing recorded (no probes, or no frames counted)".into() } else { out }
}

/// A search for the code an action runs, from Start to Stop: the functions still suspected
/// (`None`: every function, before the first round) and the rounds so far.
struct Search {
    candidates: Option<Vec<usize>>,
    rounds: u32,
    /// Every function watched at Start, and the candidates before each round (for Undo).
    all: Vec<usize>,
    before: Vec<Option<Vec<usize>>>,
}

static SEARCH: Mutex<Option<Search>> = Mutex::new(None);

/// The coverage commands. A search narrows in rounds, in any order and as many as it takes: after
/// doing the action (`action`), the candidates that ran are kept; after playing without it
/// (`inaction`), those that did not run. Each round watches what is left again.
#[cfg(target_arch = "x86_64")]
fn cover(words: &[&str]) -> Result<String, String> {
    use monaka_hook::coverage;
    let most = words.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(60);
    let list = |hits: Vec<coverage::Hit>| {
        let mut out = format!("{} functions\n", hits.len());
        for h in hits.iter().take(most) {
            let held = if h.matched == 0 { String::new() } else { format!("  value in {}", registers(h.matched)) };
            let _ = writeln!(out, "  {}  x{}{held}", Module::describe(h.start), h.hits);
        }
        out
    };
    let starts = |starts: &[usize]| {
        let mut out = String::new();
        for start in starts.iter().take(most) {
            let _ = writeln!(out, "  {}", Module::describe(*start));
        }
        out
    };
    match words.first().copied().unwrap_or("") {
        "start" => {
            let budget = words.get(1).and_then(|n| n.parse().ok()).unwrap_or(1);
            let module = module_named(words.get(2).copied().unwrap_or("exe"))?;
            let armed = coverage::arm(&module, budget)?;
            *SEARCH.locked() = Some(Search { candidates: None, rounds: 0, all: coverage::not_hit(), before: Vec::new() });
            Ok(format!("{armed} functions watched. Do the action, then Action; or play without it, then Inaction. Repeat both to narrow."))
        }
        verb @ ("action" | "inaction") => {
            let mut search = SEARCH.locked();
            let search = search.as_mut().ok_or("press Start first")?;
            // What ran since Start or the last round (each round clears the counts).
            let kept: Vec<usize> = if verb == "action" { coverage::hits().iter().map(|h| h.start).collect() } else { coverage::not_hit() };
            coverage::rearm(&kept)?;
            search.before.push(search.candidates.take());
            search.rounds += 1;
            let mut out = format!("{} candidates left after {} rounds\n", kept.len(), search.rounds);
            if kept.len() <= most {
                out += &starts(&kept);
            } else {
                let _ = writeln!(out, "(listed once {most} or fewer are left)");
            }
            search.candidates = Some(kept);
            Ok(out)
        }
        "undo" => {
            let mut search = SEARCH.locked();
            let search = search.as_mut().ok_or("press Start first")?;
            let previous = search.before.pop().ok_or("nothing to undo")?;
            // The list as it was is watched again, fresh: the undone round is done again.
            coverage::rearm(previous.as_deref().unwrap_or(&search.all))?;
            search.rounds -= 1;
            search.candidates = previous;
            let left = search.candidates.as_ref().map_or_else(|| "every function".to_owned(), |c| format!("{} candidates", c.len()));
            Ok(format!("undone: back to {left} after {} rounds; do that round again", search.rounds))
        }
        "candidates" => {
            let search = SEARCH.locked();
            match search.as_ref().map(|s| &s.candidates) {
                None => Err("no search: press Start".into()),
                Some(None) => Ok("every function (no round yet)".into()),
                Some(Some(kept)) => Ok(format!("{} candidates\n{}", kept.len(), starts(kept))),
            }
        }
        "hits" => Ok(list(coverage::hits())),
        "value" => match words.get(1).copied() {
            Some("off") | None => {
                coverage::value(None);
                Ok("value off".into())
            }
            Some(text) => {
                let value = parse_address(text, |m| module_named(m).ok().map(|m| m.base()))? as u64;
                coverage::value(Some(value));
                Ok(format!("hits check the argument registers for {value:#x}"))
            }
        },
        "matches" => Ok(list(coverage::matches())),
        "stop" => {
            *SEARCH.locked() = None;
            Ok(format!("{} tripwires out", coverage::disarm()))
        }
        _ => Err("cover start|action|inaction|undo|candidates|hits|value|matches|stop".into()),
    }
}

/// The search's state for the status: what to do next, with the candidates left.
fn search_status() -> Option<String> {
    let search = SEARCH.locked();
    let search = search.as_ref()?;
    let left = search.candidates.as_ref().map_or_else(|| "every function".to_owned(), |c| format!("{} candidates", c.len()));
    Some(format!("Find: {left}. Do the action then Action, or play without it then Inaction; Stop when done"))
}

/// "rcx, r8" for register bits (bit 0 rcx .. bit 3 r9).
#[cfg(target_arch = "x86_64")]
fn registers(bits: u8) -> String {
    ["rcx", "rdx", "r8", "r9"].iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, r)| *r).collect::<Vec<_>>().join(", ")
}

/// An error unless `address` is where a function starts (the module's exception directory says).
fn function_start(address: usize) -> Result<(), String> {
    let module = Module::containing(address).ok_or("not inside a module: add force to hook it anyway")?;
    let code = code_of(&module).ok_or("the module's code is unreadable: add force to hook it anyway")?;
    let rva = (address - module.base()) as u32;
    match code.function(rva) {
        Some(start) if start == rva => Ok(()),
        Some(start) => Err(format!("{} is inside the function at {}: hook that, or add force", Module::describe(address), Module::describe(module.base() + start as usize))),
        None => Err(format!("{} is in no function the module lists: add force to hook it anyway", Module::describe(address))),
    }
}

/// A module's code, read once.
fn code_of(module: &Module) -> Option<&'static monaka_core::scan::Code<'static>> {
    module.kept_code()
}

/// A loaded module by its name, `exe` for the game's executable.
fn module_named(name: &str) -> Result<Module, String> {
    let name = if name.eq_ignore_ascii_case("exe") { monaka_hook::module::host_name() } else { name.to_owned() };
    Module::find(&name).ok_or_else(|| format!("{name}: no such module loaded"))
}

fn strings(words: &[&str]) -> Result<String, String> {
    let text = words.first().ok_or("which text?")?;
    let module = module_named(words.get(1).copied().unwrap_or("exe"))?;
    let code = code_of(&module).ok_or("the module's code is unreadable")?;
    let texts = code.texts_containing(text, 40);
    let users = code.users(&texts);
    let at = |rva: u32| Module::describe(module.base() + rva as usize);
    let mut out = format!("{} texts\n", texts.len());
    for t in &texts {
        let _ = writeln!(out, "{}  {:?}{}", at(t.rva), t.text, if t.wide { " (UTF-16)" } else { "" });
        for (_, function, site) in users.iter().filter(|(target, _, _)| *target == t.rva).take(8) {
            let _ = writeln!(out, "    used by {} (at {})", at(*function), at(*site));
        }
    }
    Ok(out)
}

fn classes(words: &[&str]) -> Result<String, String> {
    let text = words.first().ok_or("which name?")?;
    let module = module_named(words.get(1).copied().unwrap_or("exe"))?;
    let code = code_of(&module).ok_or("the module's code is unreadable")?;
    let at = |rva: u32| Module::describe(module.base() + rva as usize);
    let found = code.classes(text, 24);
    let mut out = format!("{} classes\n", found.len());
    for class in found {
        let _ = writeln!(out, "{}", class.name);
        for table in class.tables {
            let _ = writeln!(out, "    methods at {} (offset {:#x}): {} methods", at(table.rva), table.offset, table.methods.len());
            for (k, method) in table.methods.iter().enumerate().take(12) {
                let _ = writeln!(out, "      {k:3}  {}", at(*method));
            }
        }
    }
    Ok(out)
}

fn methods(table: usize) -> Result<String, String> {
    let module = Module::containing(table).ok_or("not inside a module")?;
    let code = code_of(&module).ok_or("the module's code is unreadable")?;
    let methods = code.methods((table - module.base()) as u32);
    let mut out = format!("{} methods\n", methods.len());
    for (k, method) in methods.iter().enumerate() {
        let _ = writeln!(out, "  {k:3} (+{:#x})  {}", k * 8, Module::describe(module.base() + *method as usize));
    }
    Ok(out)
}

/// Where a value is in the bytes at an address: a float (with a dot) within a thousandth, or an
/// integer as 4 or 8 bytes.
fn find(words: &[&str]) -> Result<String, String> {
    let [address, length, value] = words else { return Err("find <addr> <bytes> <value>".into()) };
    let address = one_address(&[address])?;
    let length = number(length).ok_or("find: the size")? as usize;
    let mut bytes = vec![0u8; length.min(1 << 20)];
    if !monaka_hook::mem::read_bytes(address, &mut bytes) {
        return Err("those bytes are not readable".into());
    }
    let mut found = Vec::new();
    if value.contains('.') {
        let wanted: f32 = value.parse().map_err(|_| "find: not a float")?;
        for (k, w) in bytes.as_chunks::<4>().0.iter().enumerate() {
            let f = f32::from_le_bytes(*w);
            if (f - wanted).abs() <= 1.0e-3 * wanted.abs().max(1.0) {
                found.push(format!("+{:#x}  {f}", k * 4));
            }
        }
    } else {
        let wanted = number(value).ok_or("find: not a number")?;
        for k in (0..bytes.len().saturating_sub(3)).step_by(4) {
            let word = u32::from_le_bytes([bytes[k], bytes[k + 1], bytes[k + 2], bytes[k + 3]]);
            let long = bytes.get(k..k + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()));
            if u64::from(word) == wanted || (long == Some(wanted) && k % 8 == 0) {
                found.push(format!("+{k:#x}"));
            }
        }
    }
    Ok(format!("{} places\n{}", found.len(), found.iter().take(200).map(|f| format!("  {f}\n")).collect::<String>()))
}

/// What `spec` (`reg[+off]:bytes`, the offset and size in hex or decimal) asks to copy.
#[cfg(target_arch = "x86_64")]
fn parse_snapshot(spec: &str) -> Result<monaka_hook::live::Snapshot, String> {
    let (place, length) = spec.split_once(':').ok_or("snap=reg[+off]:bytes")?;
    let (register, offset) = match place.find(['+', '-']) {
        Some(at) => {
            let magnitude = number(&place[at + 1..]).ok_or("snap: the offset")? as isize;
            (&place[..at], if place.as_bytes()[at] == b'-' { -magnitude } else { magnitude })
        }
        None => (place, 0),
    };
    let register = monaka_hook::live::Register::parse(register).ok_or("snap: the register is rcx, rdx, r8 or r9")?;
    Ok(monaka_hook::live::Snapshot { register, offset, length: number(length).ok_or("snap: the size")? as usize })
}

/// A number, hex with `0x` or decimal.
fn number(text: &str) -> Option<u64> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// The address `text` names: `exe+0x1234` (the game's executable), `name.dll+0x1234`, or a bare
/// one; `base` finds a module's start by its name.
pub fn parse_address(text: &str, base: impl Fn(&str) -> Option<usize>) -> Result<usize, String> {
    match text.rsplit_once('+') {
        Some((module, offset)) => {
            let offset = number(offset).ok_or_else(|| format!("{offset}: not an offset"))?;
            let start = base(module).ok_or_else(|| format!("{module}: no such module loaded"))?;
            Ok(start + offset as usize)
        }
        None => number(text).map(|a| a as usize).ok_or_else(|| format!("{text}: not an address")),
    }
}

fn one_address(words: &[&str]) -> Result<usize, String> {
    let text = words.first().ok_or("which address?")?;
    parse_address(text, |module| {
        let name = if module.eq_ignore_ascii_case("exe") { monaka_hook::module::host_name() } else { module.to_owned() };
        Module::find(&name).map(|m| m.base())
    })
}

fn modules(filter: Option<&str>) -> String {
    let mut out = String::new();
    for module in Module::loaded() {
        let name = module.path().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        if filter.is_none_or(|f| name.to_ascii_lowercase().contains(&f.to_ascii_lowercase())) {
            let _ = writeln!(out, "{:#014x}  {:>10}  {name}", module.base(), module.size());
        }
    }
    out
}

fn read(address: usize, length: usize) -> String {
    let mut bytes = vec![0u8; length.min(4096)];
    if !monaka_hook::mem::read_bytes(address, &mut bytes) {
        return format!("{} is not readable", Module::describe(address));
    }
    dump(address, &bytes)
}

/// Bytes as lines of 16: their address (or offset), hex, then each 4 bytes as an integer and a
/// float.
fn dump(base: usize, bytes: &[u8]) -> String {
    let mut out = String::new();
    for (k, line) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
        let words: Vec<String> = line.as_chunks::<4>().0.iter().map(|w| format!("{:>11} {:>12.4}", u32::from_le_bytes(*w), f32::from_le_bytes(*w))).collect();
        let _ = writeln!(out, "{:#x}  {:<47}  {}", base + k * 16, hex.join(" "), words.join(" |"));
    }
    out
}

fn watch(words: &[&str]) -> Result<String, String> {
    let reads = words.last() == Some(&"reads");
    let addresses: Vec<usize> = words.iter().filter(|w| **w != "reads").map(|w| one_address(&[*w])).collect::<Result<_, _>>()?;
    let threads = if reads { monaka_hook::watch::arm_accesses(&addresses, 8) } else { monaka_hook::watch::arm(&addresses, 8) }?;
    Ok(format!("watching {} address(es) on {threads} threads; hits shows what touched them", addresses.len()))
}

fn hits() -> String {
    let hits = monaka_hook::watch::hits();
    if hits.is_empty() {
        return "no hits yet".into();
    }
    let mut out = String::new();
    for hit in hits {
        let _ = writeln!(out, "watch {}: the instruction before {} on thread {}", hit.slot, Module::describe(hit.rip), hit.thread);
        // The stack's words that point into code: the calls the write is inside, nearest first.
        for word in hit.stack.iter().filter(|w| Module::containing(**w).is_some()).take(6) {
            let _ = writeln!(out, "    {}", Module::describe(*word));
        }
    }
    out
}
