//! The numbers-and-units check: every amount in the answer must be an amount from a cited source.
//!
//! Units are translated with a fixed table, so "6 cucharaditas" matches "6 teaspoons" but
//! "6 cucharadas" (tablespoons) does not. Numbers can be digits ("2,000", "2 000", "0,5"),
//! fractions ("1/2", "½") or words ("six", "half a", "media").

use std::fmt;

use crate::text::{fold, strip_citations};

/// A unit after translation. Two amounts match only with the same unit and value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unit {
    Teaspoon,
    Tablespoon,
    /// A spoon of unknown size ("cuillère" alone).
    Spoon,
    Cup,
    Liter,
    Milliliter,
    Drop,
    Gram,
    Kilogram,
    Milligram,
    Ounce,
    Pound,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
    Meter,
    Centimeter,
    Millimeter,
    Kilometer,
    Foot,
    Inch,
    Percent,
    Celsius,
    Fahrenheit,
    Degree,
    Volt,
    Watt,
    Amp,
}

impl Unit {
    /// The English name, for messages.
    pub fn name(self) -> &'static str {
        match self {
            Unit::Teaspoon => "teaspoon",
            Unit::Tablespoon => "tablespoon",
            Unit::Spoon => "spoon",
            Unit::Cup => "cup",
            Unit::Liter => "liter",
            Unit::Milliliter => "milliliter",
            Unit::Drop => "drop",
            Unit::Gram => "gram",
            Unit::Kilogram => "kilogram",
            Unit::Milligram => "milligram",
            Unit::Ounce => "ounce",
            Unit::Pound => "pound",
            Unit::Second => "second",
            Unit::Minute => "minute",
            Unit::Hour => "hour",
            Unit::Day => "day",
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Year => "year",
            Unit::Meter => "meter",
            Unit::Centimeter => "centimeter",
            Unit::Millimeter => "millimeter",
            Unit::Kilometer => "kilometer",
            Unit::Foot => "foot",
            Unit::Inch => "inch",
            Unit::Percent => "percent",
            Unit::Celsius => "°C",
            Unit::Fahrenheit => "°F",
            Unit::Degree => "degree",
            Unit::Volt => "volt",
            Unit::Watt => "watt",
            Unit::Amp => "amp",
        }
    }
}

/// One-word units in English, French and Spanish, plurals included.
#[rustfmt::skip]
const UNIT_WORDS: &[(&[&str], Unit)] = &[
    (&["teaspoon", "teaspoons", "tsp", "tsps", "cucharadita", "cucharaditas"], Unit::Teaspoon),
    (&["tablespoon", "tablespoons", "tbsp", "tbsps", "tbs", "cucharada", "cucharadas"], Unit::Tablespoon),
    (&["spoon", "spoons", "spoonful", "spoonfuls", "cuchara", "cucharas"], Unit::Spoon),
    (&["cup", "cups", "taza", "tazas", "tasse", "tasses"], Unit::Cup),
    (&["liter", "liters", "litre", "litres", "litro", "litros", "l", "lt", "ltr"], Unit::Liter),
    (&["milliliter", "milliliters", "millilitre", "millilitres", "mililitro", "mililitros", "ml"], Unit::Milliliter),
    (&["drop", "drops", "gota", "gotas", "goutte", "gouttes"], Unit::Drop),
    (&["gram", "grams", "gramme", "grammes", "gramo", "gramos", "g", "gr"], Unit::Gram),
    (&["kilogram", "kilograms", "kilogramme", "kilogrammes", "kilogramo", "kilogramos", "kg", "kilo", "kilos"],
        Unit::Kilogram),
    (&["milligram", "milligrams", "milligramme", "milligrammes", "miligramo", "miligramos", "mg"], Unit::Milligram),
    (&["ounce", "ounces", "oz", "onza", "onzas"], Unit::Ounce),
    (&["pound", "pounds", "lb", "lbs", "libra", "libras"], Unit::Pound),
    (&["second", "seconds", "seconde", "secondes", "segundo", "segundos", "sec", "secs"], Unit::Second),
    (&["minute", "minutes", "minuto", "minutos", "min", "mins"], Unit::Minute),
    (&["hour", "hours", "heure", "heures", "hora", "horas", "hr", "hrs", "h"], Unit::Hour),
    (&["day", "days", "jour", "jours", "dia", "dias"], Unit::Day),
    (&["week", "weeks", "semaine", "semaines", "semana", "semanas"], Unit::Week),
    (&["month", "months", "mois", "mes", "meses"], Unit::Month),
    (&["year", "years", "annee", "annees", "ano", "anos"], Unit::Year),
    (&["meter", "meters", "metre", "metres", "metro", "metros", "m"], Unit::Meter),
    (&["centimeter", "centimeters", "centimetre", "centimetres", "centimetro", "centimetros", "cm"],
        Unit::Centimeter),
    (&["millimeter", "millimeters", "millimetre", "millimetres", "milimetro", "milimetros", "mm"], Unit::Millimeter),
    (&["kilometer", "kilometers", "kilometre", "kilometres", "kilometro", "kilometros", "km"], Unit::Kilometer),
    (&["foot", "feet", "ft", "pie", "pies", "pied", "pieds"], Unit::Foot),
    (&["inch", "inches", "pulgada", "pulgadas", "pouce", "pouces"], Unit::Inch),
    (&["percent", "porcentaje"], Unit::Percent),
    (&["volt", "volts", "voltio", "voltios", "v"], Unit::Volt),
    (&["watt", "watts", "vatio", "vatios", "w"], Unit::Watt),
    (&["amp", "amps", "ampere", "amperes", "amperio", "amperios"], Unit::Amp),
];

/// Number words. "un"/"una"/"une" are "a" as well as "one", but before a unit they mean one.
#[rustfmt::skip]
const NUMBER_WORDS: &[(&str, f64)] = &[
    ("one", 1.0), ("two", 2.0), ("three", 3.0), ("four", 4.0), ("five", 5.0), ("six", 6.0), ("seven", 7.0),
    ("eight", 8.0), ("nine", 9.0), ("ten", 10.0), ("eleven", 11.0), ("twelve", 12.0), ("fifteen", 15.0),
    ("twenty", 20.0), ("thirty", 30.0),
    ("un", 1.0), ("une", 1.0), ("deux", 2.0), ("trois", 3.0), ("quatre", 4.0), ("cinq", 5.0), ("sept", 7.0),
    ("huit", 8.0), ("neuf", 9.0), ("dix", 10.0), ("douze", 12.0), ("quinze", 15.0), ("vingt", 20.0),
    ("trente", 30.0),
    ("uno", 1.0), ("una", 1.0), ("dos", 2.0), ("tres", 3.0), ("cuatro", 4.0), ("cinco", 5.0), ("seis", 6.0),
    ("siete", 7.0), ("ocho", 8.0), ("nueve", 9.0), ("diez", 10.0), ("doce", 12.0), ("quince", 15.0),
    ("veinte", 20.0), ("treinta", 30.0),
];

const HALF: &[&str] = &["half", "demi", "demie", "medio", "media"];
const ARTICLES: &[&str] = &["a", "an", "un", "une", "una", "uno"];
/// Words allowed between a number and its unit: "6 level teaspoons", "6 cucharaditas rasas".
#[rustfmt::skip]
const FILLERS: &[&str] = &[
    "level", "leveled", "levelled", "full", "rasa", "rasas", "nivelada", "niveladas", "rase", "rases",
];
/// Words joining the two ends of a range: "5 to 7 cm", "1 or 2 drops", "de 5 a 7 cm".
const RANGE_WORDS: &[&str] = &["to", "or", "and", "a", "y", "o", "et", "ou"];

/// An amount: a number with a unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: Unit,
}

impl Quantity {
    pub(crate) fn same(&self, other: &Quantity) -> bool {
        self.unit == other.unit && (self.value - other.value).abs() <= 1e-9 * self.value.abs().max(1.0)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut number = format!("{:.3}", self.value);
        while number.ends_with('0') {
            number.pop();
        }
        if number.ends_with('.') {
            number.pop();
        }
        write!(f, "{number} {}", self.unit.name())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Word(String),
    Sym(char),
}

/// All amounts in a text, in order.
pub(crate) fn quantities(text: &str) -> Vec<Quantity> {
    let toks = tokenize(&fold(&strip_citations(text)));
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Some((value, after)) = value_at(&toks, i) else {
            i += 1;
            continue;
        };
        let at_unit = skip_fillers(&toks, after);
        if let Some((unit, next)) = unit_at(&toks, at_unit) {
            out.push(Quantity { value, unit });
            i = next;
            continue;
        }
        // A range shares the unit written after its second number.
        let is_range = match toks.get(after) {
            Some(Tok::Word(w)) => RANGE_WORDS.contains(&w.as_str()),
            Some(Tok::Sym(c)) => matches!(c, '-' | '\u{2013}'),
            _ => false,
        };
        if is_range {
            if let Some((second, after_second)) = value_at(&toks, after + 1) {
                if let Some((unit, next)) = unit_at(&toks, skip_fillers(&toks, after_second)) {
                    out.push(Quantity { value, unit });
                    out.push(Quantity { value: second, unit });
                    i = next;
                    continue;
                }
            }
        }
        i = after.max(i + 1);
    }
    out
}

fn skip_fillers(toks: &[Tok], mut i: usize) -> usize {
    while let Some(tok) = toks.get(i) {
        match tok {
            Tok::Word(w) if FILLERS.contains(&w.as_str()) => i += 1,
            Tok::Sym('-') => i += 1,
            _ => break,
        }
    }
    i
}

fn word(toks: &[Tok], i: usize) -> Option<&str> {
    match toks.get(i) {
        Some(Tok::Word(w)) => Some(w),
        _ => None,
    }
}

/// The number starting at `i`, and the index after it.
fn value_at(toks: &[Tok], i: usize) -> Option<(f64, usize)> {
    let (value, mut next) = match toks.get(i)? {
        Tok::Num(n) => (*n, i + 1),
        Tok::Word(w) if HALF.contains(&w.as_str()) => (0.5, i + 1),
        Tok::Word(w) => {
            let n = NUMBER_WORDS.iter().find(|(name, _)| name == w)?.1;
            // "une demi-cuillère", "un demi litro": the article belongs to the half.
            let half = skip_fillers(toks, i + 1);
            if ARTICLES.contains(&w.as_str()) && word(toks, half).is_some_and(|h| HALF.contains(&h)) {
                (0.5, half + 1)
            } else {
                (n, i + 1)
            }
        }
        Tok::Sym(_) => return None,
    };
    if value == 0.5 && word(toks, next).is_some_and(|a| ARTICLES.contains(&a)) {
        // "half a teaspoon"
        next += 1;
    } else if let (Some(and), Some(a), Some(half)) = (word(toks, next), word(toks, next + 1), word(toks, next + 2)) {
        // "1 and a half liters"
        if ["and", "y", "et"].contains(&and) && ARTICLES.contains(&a) && HALF.contains(&half) {
            return Some((value + 0.5, next + 3));
        }
    }
    Some((value, next))
}

/// The unit starting at `i`, and the index after it.
fn unit_at(toks: &[Tok], i: usize) -> Option<(Unit, usize)> {
    match toks.get(i)? {
        Tok::Sym('%') => Some((Unit::Percent, i + 1)),
        Tok::Sym('°') => match word(toks, i + 1) {
            Some("c") => Some((Unit::Celsius, i + 2)),
            Some("f") => Some((Unit::Fahrenheit, i + 2)),
            _ => Some((Unit::Degree, i + 1)),
        },
        Tok::Word(w) => {
            let w = w.as_str();
            match w {
                "cuillere" | "cuilleres" | "cuilleree" | "cuillerees" => {
                    let size = if word(toks, i + 1).is_some_and(|x| x == "a" || x == "de") {
                        i + 2
                    } else {
                        i + 1
                    };
                    match word(toks, size) {
                        Some("cafe") => Some((Unit::Teaspoon, size + 1)),
                        Some("soupe") => Some((Unit::Tablespoon, size + 1)),
                        _ => Some((Unit::Spoon, i + 1)),
                    }
                }
                "degree" | "degrees" | "degre" | "degres" | "grado" | "grados" => match word(toks, i + 1) {
                    Some("c" | "celsius" | "centigrade") => Some((Unit::Celsius, i + 2)),
                    Some("f" | "fahrenheit") => Some((Unit::Fahrenheit, i + 2)),
                    _ => Some((Unit::Degree, i + 1)),
                },
                "por" | "pour" | "per" if word(toks, i + 1).is_some_and(|x| x == "ciento" || x == "cent") => {
                    Some((Unit::Percent, i + 2))
                }
                _ => UNIT_WORDS
                    .iter()
                    .find(|(names, _)| names.contains(&w))
                    .map(|(_, unit)| (*unit, i + 1)),
            }
        }
        Tok::Num(_) | Tok::Sym(_) => None,
    }
}

fn unicode_fraction(c: char) -> Option<f64> {
    Some(match c {
        '½' => 0.5,
        '¼' => 0.25,
        '¾' => 0.75,
        '⅓' => 1.0 / 3.0,
        '⅔' => 2.0 / 3.0,
        '⅛' => 0.125,
        _ => return None,
    })
}

fn tokenize(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let (value, next) = read_number(&chars, i);
            if let Some(value) = value {
                out.push(Tok::Num(value));
            }
            i = next;
        } else if let Some(f) = unicode_fraction(c) {
            out.push(Tok::Num(f));
            i += 1;
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect()));
        } else {
            // A hyphen joins only when attached ("5-7", "demi-cuillère"); "- " starts a bullet.
            let attached = i > 0 && chars[i - 1].is_alphanumeric();
            if matches!(c, '%' | '°' | '\u{2013}') || (c == '-' && attached) {
                out.push(Tok::Sym(c));
            }
            i += 1;
        }
    }
    out
}

fn digits_from(chars: &[char], i: usize) -> usize {
    chars[i.min(chars.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count()
}

/// Reads "2,000", "2 000", "2.000" (Spanish thousands), "0.5", "0,5", "1/2" and "1½".
fn read_number(chars: &[char], start: usize) -> (Option<f64>, usize) {
    let mut i = start + digits_from(chars, start);
    let mut text: String = chars[start..i].iter().collect();
    let mut decimal = false;
    while let Some(&sep) = chars.get(i) {
        let run = digits_from(chars, i + 1);
        if run == 0 {
            break;
        }
        let int_part = !decimal && text.len() <= 3 && !text.starts_with('0');
        let group = run == 3 && int_part;
        match sep {
            ',' | '.' if group => {}
            ' ' | '\u{a0}' | '\u{202f}' if group => {}
            ',' | '.' if !decimal => {
                text.push('.');
                decimal = true;
            }
            _ => break,
        }
        text.extend(&chars[i + 1..i + 1 + run]);
        i += 1 + run;
    }
    let mut value = text.parse::<f64>().ok().filter(|v| v.is_finite());
    // A fraction: "1/2".
    if chars.get(i) == Some(&'/') {
        let run = digits_from(chars, i + 1);
        if run > 0 {
            let den: String = chars[i + 1..i + 1 + run].iter().collect();
            let den = den.parse::<f64>().ok().filter(|d| *d != 0.0 && d.is_finite());
            value = value.zip(den).map(|(n, d)| n / d);
            i += 1 + run;
        }
    }
    // A mixed number: "1½" or "1 ½".
    let gap = usize::from(chars.get(i) == Some(&' '));
    if let Some(f) = chars.get(i + gap).copied().and_then(unicode_fraction) {
        value = value.map(|v| v + f);
        i += gap + 1;
    }
    (value, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<String> {
        quantities(text).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn reads_amounts_in_three_languages() {
        assert_eq!(
            found("mix 1 liter of water, 6 level teaspoons of sugar and half a level teaspoon of salt"),
            ["1 liter", "6 teaspoon", "0.5 teaspoon"]
        );
        assert_eq!(
            found("Mezcle 1 litro, 6 cucharaditas rasas de azúcar y media cucharadita de sal"),
            ["1 liter", "6 teaspoon", "0.5 teaspoon"]
        );
        assert_eq!(
            found("6 cucharadas y media cucharada"),
            ["6 tablespoon", "0.5 tablespoon"]
        );
        assert_eq!(
            found("une demi-cuillère à café, 6 cuillères à soupe, une minute"),
            ["0.5 teaspoon", "6 tablespoon", "1 minute"]
        );
        assert_eq!(
            found("½ teaspoon, 1/2 tsp, 1½ cups"),
            ["0.5 teaspoon", "0.5 teaspoon", "1.5 cup"]
        );
    }

    #[test]
    fn reads_number_formats() {
        assert_eq!(found("above 2,000 meters (6,500 feet)"), ["2000 meter", "6500 foot"]);
        assert_eq!(found("à 2 000 mètres, a 2.000 metros"), ["2000 meter", "2000 meter"]);
        assert_eq!(found("0,5 l and 0.25 l"), ["0.5 liter", "0.25 liter"]);
        assert_eq!(found("6% bleach, 30 min"), ["6 percent", "30 minute"]);
    }

    #[test]
    fn reads_ranges() {
        assert_eq!(found("5 to 7 centimeters"), ["5 centimeter", "7 centimeter"]);
        assert_eq!(found("1-2 drops"), ["1 drop", "2 drop"]);
        // A bullet is not a range, and a citation's number is not an amount.
        assert_eq!(found("water [S1].\n- Half a teaspoon"), ["0.5 teaspoon"]);
    }

    #[test]
    fn ignores_numbers_without_units() {
        assert!(found("1. Mix it. Step 2 of 3 [S1]").is_empty());
        assert!(found("a second dose").is_empty());
    }

    #[test]
    fn survives_odd_numbers() {
        found("99999999999999999999999999999999999999999999999999 liters 1/0 cups 1e400 ml , . / ½½½");
    }
}
