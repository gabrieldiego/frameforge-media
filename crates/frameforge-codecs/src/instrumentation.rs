use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ByteCounter {
    bytes_written: usize,
}

impl ByteCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_bytes(&mut self, bytes: usize) {
        self.bytes_written += bytes;
    }

    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }
}

pub struct CountingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    bytes: ByteCounter,
}

impl<'a, W: Write + ?Sized> CountingWriter<'a, W> {
    pub fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            bytes: ByteCounter::new(),
        }
    }

    pub fn bytes_written(&self) -> usize {
        self.bytes.bytes_written()
    }
}

impl<W: Write + ?Sized> Write for CountingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.bytes.add_bytes(written);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentationCounter {
    pub name: &'static str,
    pub value: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InstrumentationCounters {
    counters: Vec<InstrumentationCounter>,
}

impl InstrumentationCounters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &'static str, delta: usize) {
        if let Some(counter) = self
            .counters
            .iter_mut()
            .find(|counter| counter.name == name)
        {
            counter.value += delta;
        } else {
            self.counters
                .push(InstrumentationCounter { name, value: delta });
        }
    }

    pub fn set(&mut self, name: &'static str, value: usize) {
        if let Some(counter) = self
            .counters
            .iter_mut()
            .find(|counter| counter.name == name)
        {
            counter.value = value;
        } else {
            self.counters.push(InstrumentationCounter { name, value });
        }
    }

    pub fn get(&self, name: &'static str) -> usize {
        self.counters
            .iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.value)
            .unwrap_or(0)
    }

    pub fn as_slice(&self) -> &[InstrumentationCounter] {
        &self.counters
    }
}

#[derive(Debug)]
pub struct JsonlInstrumentationSink {
    destination: JsonlInstrumentationDestination,
}

#[derive(Debug)]
enum JsonlInstrumentationDestination {
    File(BufWriter<File>),
    Stderr,
}

impl JsonlInstrumentationSink {
    pub fn append_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!(
                        "failed to open instrumentation destination '{}': {err}",
                        path.display()
                    ),
                )
            })?;
        Ok(Self {
            destination: JsonlInstrumentationDestination::File(BufWriter::new(file)),
        })
    }

    pub fn stderr() -> Self {
        Self {
            destination: JsonlInstrumentationDestination::Stderr,
        }
    }

    pub fn append_from_env(env_name: &str) -> io::Result<Option<Self>> {
        let Some(value) = std::env::var_os(env_name) else {
            return Ok(None);
        };
        if value.as_os_str() == OsStr::new("0") {
            return Ok(None);
        }
        if value.as_os_str() == OsStr::new("-") {
            return Ok(Some(Self::stderr()));
        }
        Self::append_file(PathBuf::from(value)).map(Some)
    }

    pub fn write_json_line(&mut self, line: &str) -> io::Result<()> {
        match &mut self.destination {
            JsonlInstrumentationDestination::File(file) => writeln!(file, "{line}"),
            JsonlInstrumentationDestination::Stderr => writeln!(io::stderr(), "{line}"),
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        match &mut self.destination {
            JsonlInstrumentationDestination::File(file) => file.flush(),
            JsonlInstrumentationDestination::Stderr => io::stderr().flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn counting_writer_tracks_successful_writes() {
        let mut output = Vec::new();
        let mut writer = CountingWriter::new(&mut output);

        writer.write_all(b"abc").unwrap();
        writer.write_all(b"de").unwrap();

        assert_eq!(writer.bytes_written(), 5);
        drop(writer);
        assert_eq!(output, b"abcde");
    }

    #[test]
    fn instrumentation_counters_add_and_set_values() {
        let mut counters = InstrumentationCounters::new();

        counters.add("partition_bits", 7);
        counters.add("partition_bits", 5);
        counters.set("residual_bits", 19);

        assert_eq!(counters.get("partition_bits"), 12);
        assert_eq!(counters.get("residual_bits"), 19);
        assert_eq!(counters.get("missing"), 0);
        assert_eq!(
            counters.as_slice(),
            &[
                InstrumentationCounter {
                    name: "partition_bits",
                    value: 12,
                },
                InstrumentationCounter {
                    name: "residual_bits",
                    value: 19,
                },
            ]
        );
    }

    #[test]
    fn jsonl_sink_appends_lines() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::current_dir()
            .unwrap()
            .join("target/frameforge-test-output");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("frameforge-instrumentation-{unique}.jsonl"));

        {
            let mut sink = JsonlInstrumentationSink::append_file(&path).unwrap();
            sink.write_json_line("{\"event\":\"first\"}").unwrap();
            sink.write_json_line("{\"event\":\"second\"}").unwrap();
            sink.flush().unwrap();
        }

        let contents = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(contents, "{\"event\":\"first\"}\n{\"event\":\"second\"}\n");
    }
}
