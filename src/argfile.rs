use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};

pub fn expand() -> Result<Vec<OsString>> {
    let mut expander = Expander::default();
    for arg in std::env::args_os() {
        expander.push(arg)?;
    }
    Ok(expander.args)
}

#[derive(Default)]
struct Expander {
    args: Vec<OsString>,
}

impl Expander {
    fn push(&mut self, arg: OsString) -> Result<()> {
        let bytes = arg.as_encoded_bytes();
        match bytes.split_first() {
            Some((b'@', rest)) => {
                self.push_file(unsafe { OsStr::from_encoded_bytes_unchecked(rest) })
            }
            _ => {
                self.args.push(arg);
                Ok(())
            }
        }
    }

    fn push_file(&mut self, file: &OsStr) -> Result<()> {
        let contents =
            std::fs::read_to_string(file).with_context(|| format!("failed to read {file:?}"))?;

        for part in imp::split(&contents) {
            self.push(part.into())?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
use gnu as imp;
#[cfg(not(windows))]
mod gnu {
    pub fn split(s: &str) -> impl Iterator<Item = String> + '_ {
        Split { iter: s.chars() }
    }

    struct Split<'a> {
        iter: std::str::Chars<'a>,
    }

    impl<'a> Iterator for Split<'a> {
        type Item = String;

        fn next(&mut self) -> Option<String> {
            loop {
                match self.iter.next()? {
                    c if c.is_whitespace() => {}
                    '"' => break Some(self.quoted('"')),
                    '\'' => break Some(self.quoted('\'')),
                    c => {
                        let mut ret = String::new();
                        self.push(&mut ret, c);
                        while let Some(next) = self.iter.next() {
                            if next.is_whitespace() {
                                break;
                            }
                            self.push(&mut ret, next);
                        }
                        break Some(ret);
                    }
                }
            }
        }
    }

    impl Split<'_> {
        fn quoted(&mut self, end: char) -> String {
            let mut part = String::new();
            while let Some(next) = self.iter.next() {
                if next == end {
                    break;
                }
                self.push(&mut part, next);
            }
            part
        }

        fn push(&mut self, dst: &mut String, ch: char) {
            if ch == '\\' {
                if let Some(ch) = self.iter.next() {
                    dst.push(ch);
                    return;
                }
            }
            dst.push(ch);
        }
    }

    #[test]
    fn tests() {
        assert_eq!(split("x").collect::<Vec<_>>(), ["x"]);
        assert_eq!(split("\\x").collect::<Vec<_>>(), ["x"]);
        assert_eq!(split("'x'").collect::<Vec<_>>(), ["x"]);
        assert_eq!(split("\"x\"").collect::<Vec<_>>(), ["x"]);

        assert_eq!(split("x y").collect::<Vec<_>>(), ["x", "y"]);
        assert_eq!(split("x\ny").collect::<Vec<_>>(), ["x", "y"]);
        assert_eq!(split("\\x y").collect::<Vec<_>>(), ["x", "y"]);
        assert_eq!(split("'x y'").collect::<Vec<_>>(), ["x y"]);
        assert_eq!(split("\"x y\"").collect::<Vec<_>>(), ["x y"]);
        assert_eq!(split("\"x 'y'\"\n'y'").collect::<Vec<_>>(), ["x 'y'", "y"]);
        assert_eq!(
            split(
                r#"
                    a\ \\b
                    z
                    "x y \\z"
                "#
            )
            .collect::<Vec<_>>(),
            ["a \\b", "z", "x y \\z"]
        );
    }
}

#[cfg(windows)]
use windows as imp;
#[cfg(windows)]
mod windows {
    pub fn split(s: &str) -> impl Iterator<Item = String> {
        winsplit::split(s).map(|s| s.to_string())
    }
}
