//! Calculatrice du lanceur : expressions, conversions d'unités, devises, couleurs.
//! Pur (std seulement) : aucun réseau, aucune E/S.
use std::collections::HashMap;
use std::f64::consts::{E, PI};

/// Taux de change, base EUR (1 EUR = x devise). Codes ISO en majuscules. Inclut "EUR" = 1.0.
#[derive(Clone, Debug, Default)]
pub struct Rates {
    pub eur: HashMap<String, f64>,
    pub date: String,
}

/// Résultat affichable.
#[derive(Clone, Debug, PartialEq)]
pub struct Calc {
    pub display: String,
    pub copy: String,
    pub color: Option<[u8; 3]>,
}

// ---------- Taux BCE ----------

/// Valeur d'un attribut XML (guillemets simples ou doubles) dans un tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let p = tag.find(&format!(" {name}="))? + name.len() + 2;
    let q = tag[p..].chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let r = &tag[p + 1..];
    Some(&r[..r.find(q)?])
}

/// Lit eurofxref-daily.xml ; None si aucun taux exploitable.
pub fn parse_ecb(xml: &str) -> Option<Rates> {
    let mut r = Rates::default();
    for t in xml.split('<').filter(|t| t.starts_with("Cube")) {
        if let Some(d) = attr(t, "time") {
            r.date = d.to_string();
        }
        let rate = attr(t, "rate").and_then(|v| v.parse::<f64>().ok());
        if let (Some(c), Some(v)) = (attr(t, "currency"), rate)
            && v > 0.0
            && v.is_finite()
        {
            r.eur.insert(c.to_uppercase(), v);
        }
    }
    if r.eur.is_empty() {
        return None;
    }
    r.eur.insert("EUR".into(), 1.0);
    Some(r)
}

// ---------- Affichage ----------

/// Nombre fini et d'ordre de grandeur affichable.
fn fin(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() < 1e15).then_some(v)
}

/// `sig` chiffres significatifs (au moins ceux de la partie entière), sans zéros traînants, point décimal.
fn num(v: f64, sig: usize) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let ent = if v.abs() >= 1.0 { v.abs().log10().floor() as usize + 1 } else { 0 };
    let arrondi: f64 = format!("{:.*e}", sig.max(ent) - 1, v).parse().unwrap_or(v);
    format!("{arrondi}")
}

/// Passe au format français : virgule décimale, espace fine insécable pour les milliers.
fn fr(s: &str) -> String {
    let (neg, s) = s.strip_prefix('-').map_or(("", s), |r| ("-", r));
    let (i, f) = s.split_once('.').map_or((s, None), |(i, f)| (i, Some(f)));
    let mut out = String::from(neg);
    for (k, c) in i.chars().enumerate() {
        if k > 0 && (i.len() - k) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    if let Some(f) = f {
        out.push(',');
        out.push_str(f);
    }
    out
}

fn simple(display: String, copy: String) -> Calc {
    Calc { display, copy, color: None }
}

// ---------- Expressions ----------

#[derive(Clone, PartialEq)]
enum T {
    N(f64),
    I(String),
    O(char),
}

fn lex(s: &str) -> Option<Vec<T>> {
    let c: Vec<char> = s.chars().collect();
    let (mut i, mut t) = (0, vec![]);
    let nb = |x: char| x.is_ascii_digit() || x == '.' || x == ',';
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if nb(ch) {
            let j = (i..c.len()).find(|&j| !nb(c[j])).unwrap_or(c.len());
            let w: String = c[i..j].iter().map(|&x| if x == ',' { '.' } else { x }).collect();
            t.push(T::N(w.parse().ok()?));
            i = j;
        } else if ch.is_alphabetic() {
            let j = (i..c.len()).find(|&j| !c[j].is_alphabetic()).unwrap_or(c.len());
            t.push(T::I(c[i..j].iter().collect()));
            i = j;
        } else {
            t.push(T::O(match ch {
                '×' => '*',
                '÷' => '/',
                '+' | '-' | '*' | '/' | '^' | '%' | '(' | ')' => ch,
                _ => return None,
            }));
            i += 1;
        }
    }
    Some(t)
}

/// Descente récursive. `ops` : au moins un opérateur/fonction/constante/% (sinon ce n'est pas un calcul).
struct P {
    t: Vec<T>,
    i: usize,
    d: u32,
    ops: bool,
}

impl P {
    fn op(&mut self, set: &str) -> Option<char> {
        if let Some(T::O(c)) = self.t.get(self.i) {
            let c = *c;
            if set.contains(c) {
                self.i += 1;
                return Some(c);
            }
        }
        None
    }
    fn add(&mut self) -> Option<f64> {
        let mut a = self.mul()?;
        while let Some(o) = self.op("+-") {
            self.ops = true;
            let s = self.i;
            let mut b = self.mul()?;
            // "200 + 10%" : le pourcentage porte sur la partie gauche
            if self.i - s == 2 && matches!(self.t[s], T::N(_)) && self.t[s + 1] == T::O('%') {
                b *= a;
            }
            a = if o == '+' { a + b } else { a - b };
        }
        Some(a)
    }
    fn mul(&mut self) -> Option<f64> {
        let mut a = self.unary()?;
        loop {
            let o = match (self.op("*/"), self.t.get(self.i)) {
                (Some(o), _) => o,
                (None, Some(T::I(w))) if w == "mod" => {
                    self.i += 1;
                    'm'
                }
                _ => return Some(a),
            };
            self.ops = true;
            let b = self.unary()?;
            if b == 0.0 && o != '*' {
                return None;
            }
            a = match o {
                '*' => a * b,
                '/' => a / b,
                _ => a % b,
            };
        }
    }
    fn unary(&mut self) -> Option<f64> {
        self.d += 1;
        if self.d > 64 {
            return None; // garde-fou contre les imbrications absurdes
        }
        let r = match self.op("+-") {
            Some(o) => {
                let v = self.unary()?;
                if o == '-' { -v } else { v }
            }
            None => self.pow()?,
        };
        self.d -= 1;
        Some(r)
    }
    fn pow(&mut self) -> Option<f64> {
        let a = self.post()?;
        if self.op("^").is_some() {
            self.ops = true;
            return Some(a.powf(self.unary()?)); // associatif à droite
        }
        Some(a)
    }
    fn post(&mut self) -> Option<f64> {
        let mut a = self.atom()?;
        while self.op("%").is_some() {
            self.ops = true;
            a /= 100.0;
        }
        Some(a)
    }
    fn atom(&mut self) -> Option<f64> {
        let t = self.t.get(self.i)?.clone();
        self.i += 1;
        match t {
            T::N(v) => Some(v),
            T::O('(') => {
                let v = self.add()?;
                self.op(")")?;
                Some(v)
            }
            T::I(w) => {
                self.ops = true;
                match w.as_str() {
                    "pi" => return Some(PI),
                    "e" => return Some(E),
                    _ => {}
                }
                self.op("(")?;
                let x = self.add()?;
                self.op(")")?;
                Some(match w.as_str() {
                    "sqrt" => x.sqrt(),
                    "abs" => x.abs(),
                    "round" => x.round(),
                    "floor" => x.floor(),
                    "ceil" => x.ceil(),
                    "sin" => x.sin(),
                    "cos" => x.cos(),
                    "tan" => x.tan(),
                    "ln" => x.ln(),
                    "log" => x.log10(),
                    _ => return None,
                })
            }
            _ => None,
        }
    }
}

/// (valeur, est-un-vrai-calcul)
fn raw(s: &str) -> Option<(f64, bool)> {
    let t = lex(&s.to_lowercase())?;
    let n = t.len();
    let mut p = P { t, i: 0, d: 0, ops: false };
    let v = p.add()?;
    (p.i == n).then_some((v, p.ops && n > 1)) // "42" ou "pi" seuls ne sont pas des calculs
}

/// "15% de 80", "15% of 80"
fn pct_of(q: &str) -> Option<f64> {
    let q = q.to_lowercase();
    let (l, r) = q.split_once(" de ").or_else(|| q.split_once(" of "))?;
    Some(raw(l.trim().strip_suffix('%')?)?.0 * raw(r)?.0 / 100.0)
}

fn calc(q: &str) -> Option<Calc> {
    let v = match pct_of(q) {
        Some(v) => v,
        None => {
            let (v, ok) = raw(q)?;
            ok.then_some(v)?
        }
    };
    let v = fin(v)?;
    Some(simple(format!("= {}", fr(&num(v, 10))), num(v, 10)))
}

// ---------- Unités ----------

// (noms, catégorie, facteur, décalage, nom affiché) ; base = (v + décalage) * facteur.
// Catégories : L longueur (m) M masse (g) T température (°C) V volume (l)
// S vitesse (m/s) D données (octet) H temps (s) A surface (m2)
type Row = (&'static [&'static str], char, f64, f64, &'static str);
const U: &[Row] = &[
    (&["mm"], 'L', 0.001, 0.0, "mm"),
    (&["cm"], 'L', 0.01, 0.0, "cm"),
    (&["m"], 'L', 1.0, 0.0, "m"),
    (&["km"], 'L', 1000.0, 0.0, "km"),
    (&["in", "inch", "inches", "pouce"], 'L', 0.0254, 0.0, "in"),
    (&["ft", "feet", "foot", "pied"], 'L', 0.3048, 0.0, "ft"),
    (&["yd", "yard"], 'L', 0.9144, 0.0, "yd"),
    (&["mi", "mile"], 'L', 1609.344, 0.0, "mi"),
    (&["mg"], 'M', 0.001, 0.0, "mg"),
    (&["g"], 'M', 1.0, 0.0, "g"),
    (&["kg"], 'M', 1000.0, 0.0, "kg"),
    (&["t"], 'M', 1e6, 0.0, "t"),
    (&["oz"], 'M', 28.349523125, 0.0, "oz"),
    (&["lb", "livre"], 'M', 453.59237, 0.0, "lb"),
    (&["c", "°c", "celsius"], 'T', 1.0, 0.0, "°C"),
    (&["f", "°f", "fahrenheit"], 'T', 5.0 / 9.0, -32.0, "°F"),
    (&["k", "kelvin"], 'T', 1.0, -273.15, "K"),
    (&["ml"], 'V', 0.001, 0.0, "ml"),
    (&["cl"], 'V', 0.01, 0.0, "cl"),
    (&["l", "litre"], 'V', 1.0, 0.0, "l"),
    (&["gal"], 'V', 3.785411784, 0.0, "gal"),
    (&["floz"], 'V', 0.0295735295625, 0.0, "fl oz"),
    (&["tasse"], 'V', 0.25, 0.0, "tasse"),
    (&["kmh", "km/h"], 'S', 1.0 / 3.6, 0.0, "km/h"),
    (&["mph"], 'S', 0.44704, 0.0, "mph"),
    (&["ms", "m/s"], 'S', 1.0, 0.0, "m/s"),
    (&["noeud", "nœud", "kn"], 'S', 1852.0 / 3600.0, 0.0, "kn"),
    (&["o", "octet"], 'D', 1.0, 0.0, "o"),
    (&["b", "byte"], 'D', 1.0, 0.0, "B"),
    (&["ko", "kio"], 'D', 1024.0, 0.0, "Ko"),
    (&["kb"], 'D', 1024.0, 0.0, "KB"),
    (&["mo", "mio"], 'D', 1048576.0, 0.0, "Mo"),
    (&["mb"], 'D', 1048576.0, 0.0, "MB"),
    (&["go", "gio"], 'D', 1073741824.0, 0.0, "Go"),
    (&["gb"], 'D', 1073741824.0, 0.0, "GB"),
    (&["to", "tio"], 'D', 1099511627776.0, 0.0, "To"),
    (&["tb"], 'D', 1099511627776.0, 0.0, "TB"),
    (&["bit"], 'D', 0.125, 0.0, "bit"),
    (&["kbit"], 'D', 125.0, 0.0, "kbit"),
    (&["mbit"], 'D', 125_000.0, 0.0, "mbit"),
    (&["gbit"], 'D', 125_000_000.0, 0.0, "gbit"),
    (&["ms"], 'H', 0.001, 0.0, "ms"),
    (&["s", "sec", "seconde"], 'H', 1.0, 0.0, "s"),
    (&["min"], 'H', 60.0, 0.0, "min"),
    (&["h", "heure"], 'H', 3600.0, 0.0, "h"),
    (&["j", "jour", "day"], 'H', 86400.0, 0.0, "j"),
    (&["sem", "semaine", "week"], 'H', 604800.0, 0.0, "sem"),
    (&["an", "year"], 'H', 31557600.0, 0.0, "an"),
    (&["m2", "m²"], 'A', 1.0, 0.0, "m2"),
    (&["km2", "km²"], 'A', 1e6, 0.0, "km2"),
    (&["ha"], 'A', 1e4, 0.0, "ha"),
    (&["acre"], 'A', 4046.8564224, 0.0, "acre"),
];

/// Lignes de la table correspondant à `s` (avec tolérance du pluriel).
fn units(s: &str) -> Vec<&'static Row> {
    let f = |k: &str| U.iter().filter(|r| r.0.iter().any(|n| *n == k)).collect::<Vec<_>>();
    let v = f(s);
    match s.strip_suffix('s') {
        Some(t) if v.is_empty() => f(t),
        _ => v,
    }
}

/// "50€" / "€50" / "10 km" -> (50.0, "€")
fn split_num(a: &str) -> Option<(f64, String)> {
    let a = a.trim();
    let a = match a.chars().next().filter(|c| "€$£¥".contains(*c)) {
        Some(c) => format!("{} {c}", &a[c.len_utf8()..]),
        None => a.to_string(),
    };
    let end = a
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || c == ',' || (i == 0 && c == '-')))
        .map_or(a.len(), |(i, _)| i);
    let v = a[..end].replace(',', ".").parse().ok()?;
    let u: String = a[end..].chars().filter(|c| !c.is_whitespace()).collect();
    (!u.is_empty()).then_some((v, u))
}

fn currency(u: &str) -> Option<String> {
    Some(match u {
        "€" => "EUR".into(),
        "$" => "USD".into(),
        "£" => "GBP".into(),
        "¥" => "JPY".into(),
        _ if u.len() == 3 && u.chars().all(|c| c.is_ascii_alphabetic()) => u.to_uppercase(),
        _ => return None,
    })
}

fn conv2(a: &str, b: &str, rates: &Rates) -> Option<Calc> {
    let (v, ua) = split_num(a)?;
    let ub: String = b.chars().filter(|c| !c.is_whitespace()).collect();
    for x in units(&ua) {
        for y in units(&ub) {
            if x.1 == y.1 {
                let r = fin((v + x.3) * x.2 / y.2 - y.3)?;
                return Some(simple(
                    format!("{} {} = {} {}", fr(&num(v, 10)), x.4, fr(&num(r, 4)), y.4),
                    num(r, 4),
                ));
            }
        }
    }
    let (from, to) = (currency(&ua)?, currency(&ub)?);
    let (f, t) = (*rates.eur.get(&from)?, *rates.eur.get(&to)?);
    let r = fin(v / f * t)?;
    let date = if rates.date.is_empty() { String::new() } else { format!(" (taux du {})", rates.date) };
    let r2 = format!("{r:.2}");
    Some(simple(format!("{} {from} = {} {to}{date}", fr(&num(v, 10)), fr(&r2)), r2))
}

/// "<nombre> <unité> en|to|in|-> <unité>", unités ou devises.
fn convert(q: &str, rates: &Rates) -> Option<Calc> {
    let l = q.to_lowercase();
    for sep in ["->", " en ", " to ", " in "] {
        for (i, _) in l.match_indices(sep) {
            if let Some(c) = conv2(&l[..i], &l[i + sep.len()..], rates) {
                return Some(c);
            }
        }
    }
    None
}

// ---------- Couleurs ----------

fn hsl(c: [u8; 3]) -> (i32, i32, i32) {
    let [r, g, b] = c.map(|x| x as f64 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let (l, d) = ((mx + mn) / 2.0, mx - mn);
    if d == 0.0 {
        return (0, 0, (l * 100.0).round() as i32);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = 60.0
        * if mx == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
    ((h.round() as i32) % 360, (s * 100.0).round() as i32, (l * 100.0).round() as i32)
}

/// "#rgb", "#rrggbb" ou "rgb(r, g, b)". Le "#" est obligatoire pour l'hexadécimal.
fn color(q: &str) -> Option<Calc> {
    let l = q.to_lowercase();
    let c: [u8; 3] = if let Some(h) = l.strip_prefix('#') {
        let d: Vec<u8> = h.chars().filter_map(|c| c.to_digit(16)).map(|x| x as u8).collect();
        if d.len() != h.chars().count() {
            return None;
        }
        match d[..] {
            [a, b, c] => [a * 17, b * 17, c * 17],
            [a, b, c, d, e, f] => [a * 16 + b, c * 16 + d, e * 16 + f],
            _ => return None,
        }
    } else {
        let a = l.strip_prefix("rgb(")?.strip_suffix(')')?;
        let p: Vec<u8> = a.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
        match p[..] {
            [r, g, b] => [r, g, b],
            _ => return None,
        }
    };
    let hex = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    let (h, s, li) = hsl(c);
    Some(Calc {
        display: format!("{hex}  rgb({}, {}, {})  hsl({h}°, {s}%, {li}%)", c[0], c[1], c[2]),
        copy: hex,
        color: Some(c),
    })
}

// ---------- Point d'entrée ----------

/// None si la requête n'est pas un calcul (pour ne pas polluer les résultats du lanceur).
pub fn eval(q: &str, rates: &Rates) -> Option<Calc> {
    let q = q.trim();
    color(q).or_else(|| convert(q, rates)).or_else(|| calc(q))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<?xml version='1.0'?><gesmes:Envelope><Cube><Cube time='2026-11-20'>\
        <Cube currency='USD' rate='1.0812'/><Cube currency=\"JPY\" rate=\"162.5\"/>\
        <Cube currency='XXX' rate='abc'/></Cube></Cube></gesmes:Envelope>";

    fn rates() -> Rates {
        parse_ecb(XML).unwrap()
    }
    fn d(q: &str) -> Option<String> {
        eval(q, &rates()).map(|c| c.display)
    }
    fn cp(q: &str) -> Option<String> {
        eval(q, &rates()).map(|c| c.copy)
    }

    #[test]
    fn ecb() {
        let r = rates();
        assert_eq!(r.date, "2026-11-20");
        assert_eq!(r.eur["EUR"], 1.0);
        assert_eq!(r.eur["USD"], 1.0812);
        assert!(!r.eur.contains_key("XXX"));
        assert!(parse_ecb("<xml/>").is_none());
        assert!(parse_ecb("").is_none());
    }

    #[test]
    fn operateurs() {
        assert_eq!(d("1+2*3").unwrap(), "= 7");
        assert_eq!(d("(1+2)*3").unwrap(), "= 9");
        assert_eq!(d("2^3^2").unwrap(), "= 512");
        assert_eq!(d("-2^2").unwrap(), "= -4");
        assert_eq!(d("2^-1").unwrap(), "= 0,5");
        assert_eq!(d("10 mod 3").unwrap(), "= 1");
        assert_eq!(d("6×7").unwrap(), "= 42");
        assert_eq!(d("84÷2").unwrap(), "= 42");
        assert_eq!(d("1/3").unwrap(), "= 0,3333333333");
        assert_eq!(d("0.1+0.2").unwrap(), "= 0,3");
    }

    #[test]
    fn virgule_et_milliers() {
        assert_eq!(d("2,5*2").unwrap(), "= 5");
        assert_eq!(d("1000*1.2345").unwrap(), "= 1\u{202f}234,5");
        assert_eq!(cp("1000*1.2345").unwrap(), "1234.5");
        assert_eq!(d("1000000*3").unwrap(), "= 3\u{202f}000\u{202f}000");
        assert!(d("1 234 + 1").is_none());
    }

    #[test]
    fn fonctions_et_constantes() {
        assert_eq!(d("sqrt(16)+1").unwrap(), "= 5");
        assert_eq!(d("abs(-3)*2").unwrap(), "= 6");
        assert_eq!(d("round(2.5)+floor(1.9)+ceil(0.1)").unwrap(), "= 5");
        assert_eq!(d("log(1000)+ln(e)").unwrap(), "= 4");
        assert_eq!(d("pi*2").unwrap(), "= 6,283185307");
        assert_eq!(d("sin(0)+cos(0)+tan(0)").unwrap(), "= 1");
        assert_eq!(d("SQRT(9)*1").unwrap(), "= 3");
    }

    #[test]
    fn pourcentages() {
        assert_eq!(d("10%").unwrap(), "= 0,1");
        assert_eq!(d("200 + 10%").unwrap(), "= 220");
        assert_eq!(d("200 - 10%").unwrap(), "= 180");
        assert_eq!(d("15% de 80").unwrap(), "= 12");
        assert_eq!(d("15% of 80").unwrap(), "= 12");
        assert_eq!(cp("15% de 80").unwrap(), "12");
    }

    #[test]
    fn pas_un_calcul() {
        for q in [
            "", "chrome", "42", "-5", "steam", "d lanceur", "#zz", "ff8800", "pi", "e", "1/0", "1 mod 0",
            "2 +", "(1+2", "1+2)", "sqrt(-1)", "ln(0)", "abs", "2 3", "5 % 3", "firefox 2", "#ff88",
            "rgb(256,0,0)", "rgb(1,2)", "10 kg en km", "10 xyz en km", "km en mi", "1e999999*0",
        ] {
            assert!(d(q).is_none(), "devrait être None : {q:?}");
        }
        assert!(d(&"(".repeat(100_000)).is_none());
        assert!(d(&format!("{}1", "-".repeat(100_000))).is_none());
        assert!(d(&format!("{}*2", "9".repeat(400))).is_none());
    }

    #[test]
    fn unites() {
        assert_eq!(d("10 km en mi").unwrap(), "10 km = 6,214 mi");
        assert_eq!(cp("10 km en mi").unwrap(), "6.214");
        assert_eq!(d("10KM TO MI").unwrap(), "10 km = 6,214 mi");
        assert_eq!(d("10km->mi").unwrap(), "10 km = 6,214 mi");
        assert_eq!(d("5 miles in km").unwrap(), "5 mi = 8,047 km");
        assert_eq!(d("2,5 kg en lb").unwrap(), "2,5 kg = 5,512 lb");
        assert_eq!(d("2 livres en g").unwrap(), "2 lb = 907,2 g");
        assert_eq!(d("12 pouces en ft").unwrap(), "12 in = 1 ft");
        assert_eq!(d("1 km en mm").unwrap(), "1 km = 1\u{202f}000\u{202f}000 mm");
        assert_eq!(d("100 c en f").unwrap(), "100 °C = 212 °F");
        assert_eq!(d("32 °f en c").unwrap(), "32 °F = 0 °C");
        assert_eq!(d("0 c to k").unwrap(), "0 °C = 273,1 K");
        assert_eq!(d("1 tasse en ml").unwrap(), "1 tasse = 250 ml");
        assert_eq!(d("100 kmh en mph").unwrap(), "100 km/h = 62,14 mph");
        assert_eq!(d("10 ms en kmh").unwrap(), "10 m/s = 36 km/h");
        assert_eq!(d("1500 ms en s").unwrap(), "1\u{202f}500 ms = 1,5 s");
        assert_eq!(d("1 go en mo").unwrap(), "1 Go = 1\u{202f}024 Mo");
        assert_eq!(d("1 gio en mio").unwrap(), "1 Go = 1\u{202f}024 Mo");
        assert_eq!(d("8 bit en o").unwrap(), "8 bit = 1 o");
        assert_eq!(d("1 gbit en mbit").unwrap(), "1 gbit = 1\u{202f}000 mbit");
        assert_eq!(d("90 min en h").unwrap(), "90 min = 1,5 h");
        assert_eq!(d("2 semaines en jours").unwrap(), "2 sem = 14 j");
        assert_eq!(d("1 ha en m2").unwrap(), "1 ha = 10\u{202f}000 m2");
        assert_eq!(d("1 acre en m²").unwrap(), "1 acre = 4\u{202f}047 m2");
    }

    #[test]
    fn devises() {
        assert_eq!(d("100 usd en eur").unwrap(), "100 USD = 92,49 EUR (taux du 2026-11-20)");
        assert_eq!(cp("100 usd en eur").unwrap(), "92.49");
        assert_eq!(d("50€ en $").unwrap(), "50 EUR = 54,06 USD (taux du 2026-11-20)");
        assert_eq!(d("$50 en eur").unwrap(), "50 USD = 46,24 EUR (taux du 2026-11-20)");
        assert_eq!(d("20 eur to jpy").unwrap(), "20 EUR = 3\u{202f}250,00 JPY (taux du 2026-11-20)");
        assert!(d("100 usd en xyz").is_none());
        assert!(d("100 min en eur").is_none());
        assert!(eval("100 usd en eur", &Rates::default()).is_none());
        let mut r = rates();
        r.date.clear();
        assert_eq!(eval("1 eur en eur", &r).unwrap().display, "1 EUR = 1,00 EUR");
    }

    #[test]
    fn couleurs() {
        let want = "#FF8800  rgb(255, 136, 0)  hsl(32°, 100%, 50%)";
        for q in ["#ff8800", "#F80", "rgb(255,136,0)", " RGB( 255 , 136 , 0 ) "] {
            let c = eval(q, &rates()).unwrap();
            assert_eq!((c.display.as_str(), c.copy.as_str(), c.color), (want, "#FF8800", Some([255, 136, 0])), "{q}");
        }
        let n = eval("#000000", &rates()).unwrap();
        assert_eq!(n.display, "#000000  rgb(0, 0, 0)  hsl(0°, 0%, 0%)");
        assert_eq!(eval("#0000ff", &rates()).unwrap().display, "#0000FF  rgb(0, 0, 255)  hsl(240°, 100%, 50%)");
        assert!(eval("2+2", &rates()).unwrap().color.is_none());
    }
}
