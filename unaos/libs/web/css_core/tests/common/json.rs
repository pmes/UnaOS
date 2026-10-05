//! A minimal JSON reader for the test vectors (zero dependencies).
#![allow(dead_code)]

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a,
            _ => &[],
        }
    }
    pub fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            _ => "",
        }
    }
    pub fn num(&self) -> f64 {
        match self {
            Json::Num(n) => *n,
            _ => f64::NAN,
        }
    }
    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(o) => o.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
}

pub fn parse(s: &str) -> Json {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let v = value(&c, &mut i);
    v
}

fn ws(c: &[char], i: &mut usize) {
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
}

fn value(c: &[char], i: &mut usize) -> Json {
    ws(c, i);
    match c[*i] {
        '[' => {
            *i += 1;
            let mut out = Vec::new();
            loop {
                ws(c, i);
                if c[*i] == ']' {
                    *i += 1;
                    return Json::Arr(out);
                }
                out.push(value(c, i));
                ws(c, i);
                if c[*i] == ',' {
                    *i += 1;
                }
            }
        }
        '{' => {
            *i += 1;
            let mut out = Vec::new();
            loop {
                ws(c, i);
                if c[*i] == '}' {
                    *i += 1;
                    return Json::Obj(out);
                }
                let k = match value(c, i) {
                    Json::Str(s) => s,
                    _ => panic!("json key"),
                };
                ws(c, i);
                assert_eq!(c[*i], ':');
                *i += 1;
                out.push((k, value(c, i)));
                ws(c, i);
                if c[*i] == ',' {
                    *i += 1;
                }
            }
        }
        '"' => {
            *i += 1;
            let mut s = String::new();
            loop {
                let ch = c[*i];
                *i += 1;
                match ch {
                    '"' => return Json::Str(s),
                    '\\' => {
                        let e = c[*i];
                        *i += 1;
                        match e {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            'b' => s.push('\u{8}'),
                            'f' => s.push('\u{c}'),
                            'u' => {
                                let h = |c: &[char], i: &mut usize| {
                                    let v = u32::from_str_radix(&c[*i..*i + 4].iter().collect::<String>(), 16).unwrap();
                                    *i += 4;
                                    v
                                };
                                let v = h(c, i);
                                if (0xD800..0xDC00).contains(&v) && c.get(*i) == Some(&'\\') && c.get(*i + 1) == Some(&'u') {
                                    *i += 2;
                                    let lo = h(c, i);
                                    s.push(char::from_u32(0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00)).unwrap_or('\u{FFFD}'));
                                } else {
                                    s.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                                }
                            }
                            o => s.push(o),
                        }
                    }
                    o => s.push(o),
                }
            }
        }
        't' => {
            *i += 4;
            Json::Bool(true)
        }
        'f' => {
            *i += 5;
            Json::Bool(false)
        }
        'n' => {
            *i += 4;
            Json::Null
        }
        _ => {
            let st = *i;
            while *i < c.len() && (c[*i].is_ascii_digit() || "+-.eE".contains(c[*i])) {
                *i += 1;
            }
            Json::Num(c[st..*i].iter().collect::<String>().parse().unwrap())
        }
    }
}
