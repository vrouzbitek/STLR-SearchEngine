//! Inline calculator: tiny recursive-descent parser, no dependencies.
//! Grammar: expr = term (+|- term)* ; term = power (*|/ power)* ; power = unary (^ power)? ;
//!          unary = -unary | postfix ; postfix = primary %* ; primary = number | ( expr )
//! Also: "15% of 300" / "15% z 300". Decimal comma is accepted ("2,5*4").

/// Result formatted for display, or None if the input isn't a (non-trivial) math expression.
pub fn eval(input: &str) -> Option<String> {
    let s = input.trim().to_lowercase().replace(',', ".").replace('×', "*").replace('÷', "/");
    let value = if let Some((a, b)) = s.split_once("% of ").or_else(|| s.split_once("% z ")) {
        parse(a)? / 100.0 * parse(b)?
    } else {
        // A bare number ("250", "-5") is not a calculation.
        let ops = s.trim_start_matches('-');
        if !ops.contains(['+', '-', '*', '/', '^', '%']) {
            return None;
        }
        parse(&s)?
    };
    value.is_finite().then(|| format(value))
}

fn parse(s: &str) -> Option<f64> {
    if !s.chars().all(|c| c.is_ascii_digit() || " .+-*/^%()".contains(c)) || !s.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

fn format(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i) == Some(&b' ') {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.b.get(self.i) == Some(&c);
        self.i += hit as usize;
        hit
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                v += self.term()?;
            } else if self.eat(b'-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            if self.eat(b'*') {
                v *= self.power()?;
            } else if self.eat(b'/') {
                let d = self.power()?;
                if d == 0.0 {
                    return None;
                }
                v /= d;
            } else {
                return Some(v);
            }
        }
    }
    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat(b'^') {
            return Some(base.powf(self.power()?)); // right-associative
        }
        Some(base)
    }
    fn unary(&mut self) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.unary()?);
        }
        let mut v = self.primary()?;
        while self.eat(b'%') {
            v /= 100.0;
        }
        Some(v)
    }
    fn primary(&mut self) -> Option<f64> {
        if self.eat(b'(') {
            let v = self.expr()?;
            return self.eat(b')').then_some(v);
        }
        self.ws();
        let start = self.i;
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::eval;

    #[test]
    fn evaluates() {
        assert_eq!(eval("250*1.23").as_deref(), Some("307.5"));
        assert_eq!(eval("(120+45)/2").as_deref(), Some("82.5"));
        assert_eq!(eval("15% of 300").as_deref(), Some("45"));
        assert_eq!(eval("15% z 300").as_deref(), Some("45"));
        assert_eq!(eval("2^3^2").as_deref(), Some("512"));
        assert_eq!(eval("-3 + 5").as_deref(), Some("2"));
        assert_eq!(eval("2,5*4").as_deref(), Some("10"));
        assert_eq!(eval("10/3").as_deref(), Some("3.3333333333"));
        assert_eq!(eval("50%").as_deref(), Some("0.5"));
        assert_eq!(eval("2 * (3 + 4)").as_deref(), Some("14"));
    }

    #[test]
    fn rejects_non_math() {
        for s in ["250", "-5", "abc", "playboi carti", "1/0", "2+", "(1+2", "2 ++ 3", "", "stellarclth.pl", "1.2.3+1"] {
            assert_eq!(eval(s), None, "{s:?}");
        }
    }
}
