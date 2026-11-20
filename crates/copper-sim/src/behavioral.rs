//! Nonlinear elements with a general law: behavioural sources and the
//! branches of the semiconductor devices.
//!
//! # Laws
//!
//! A [`Law`] element ([`crate::circuit::ElementKind::Law`]) sits between two
//! terminals `a` and `b` and is controlled by a list of [`Control`]s: node
//! voltage differences `V(p) - V(n)` and currents of other elements. Its
//! [`Output`] is either
//!
//! - a **current** from `a` through the element to `b`,
//!   `I = f(u) + d/dt q(u)`, with an optional charge `q` (the charge of a
//!   junction or a channel), or
//! - a **voltage** `V(a) - V(b) = f(u)` with the element's current as a
//!   branch unknown, as a voltage source.
//!
//! `f` (and `q`) is a [`Function`]: an [`Expr`] (the behavioural sources of a
//! SPICE netlist: `VALUE`, `TABLE`, `POLY`, `B` sources) or a branch of a
//! semiconductor device ([`crate::semiconductors`]).
//!
//! # Derivatives
//!
//! Newton iteration and the small-signal analyses need `df/du`. Every
//! function is evaluated in forward-mode automatic differentiation
//! ([`Dual`]): each value carries its exact partial derivatives with respect
//! to the controls, propagated by the chain rule through every operation, so
//! the derivatives are analytic, not finite differences. At points where a
//! function is not differentiable (the corner of `abs`, `limit`, `min`,
//! `max`, a `TABLE` breakpoint, the switch of an `if`) the derivative is the
//! one of the side the value is taken from; comparisons, `sgn`, `u()`,
//! `floor`, `ceil`, `round` and `int` have derivative zero. `exp` continues
//! linearly above an argument of 80 (as the diode's exponential does), so a
//! far-off Newton iterate cannot overflow; below that it is exact.
//!
//! # Convergence aids
//!
//! Each control may carry a [`Limit`]: junction controls are limited per
//! Newton iteration as SPICE's `pnjlim` does, FET gate controls as `fetlim`.
//! A behavioural source has no limits; its controls are linearised at the
//! present iterate.

use crate::circuit::{ElementId, NodeId};
use std::sync::Arc;

/// Exponent above which `exp` continues linearly.
pub const EXP_LIMIT: f64 = 80.0;

/// A value and its partial derivatives with respect to `n` variables.
#[derive(Debug, Clone, PartialEq)]
pub struct Dual {
    /// The value.
    pub v: f64,
    /// `d value / d variable_k`.
    pub d: Vec<f64>,
}

impl Dual {
    /// A constant of `n` variables.
    pub fn constant(v: f64, n: usize) -> Dual {
        Dual { v, d: vec![0.0; n] }
    }

    /// Variable `k` of `n`, at value `v`.
    pub fn variable(v: f64, k: usize, n: usize) -> Dual {
        let mut d = vec![0.0; n];
        d[k] = 1.0;
        Dual { v, d }
    }

    /// The function of `self` with value `f` and derivative `df` (chain rule).
    pub fn chain(&self, f: f64, df: f64) -> Dual {
        Dual { v: f, d: self.d.iter().map(|x| x * df).collect() }
    }

    fn zip(&self, other: &Dual, v: f64, da: f64, db: f64) -> Dual {
        let n = self.d.len().max(other.d.len());
        let get = |d: &[f64], k: usize| d.get(k).copied().unwrap_or(0.0);
        Dual { v, d: (0..n).map(|k| da * get(&self.d, k) + db * get(&other.d, k)).collect() }
    }

    /// `self + other`.
    pub fn add(&self, other: &Dual) -> Dual {
        self.zip(other, self.v + other.v, 1.0, 1.0)
    }
    /// `self - other`.
    pub fn sub(&self, other: &Dual) -> Dual {
        self.zip(other, self.v - other.v, 1.0, -1.0)
    }
    /// `self * other`.
    pub fn mul(&self, other: &Dual) -> Dual {
        self.zip(other, self.v * other.v, other.v, self.v)
    }
    /// `self / other`.
    pub fn div(&self, other: &Dual) -> Dual {
        let q = self.v / other.v;
        self.zip(other, q, 1.0 / other.v, -q / other.v)
    }
    /// `-self`.
    pub fn neg(&self) -> Dual {
        self.chain(-self.v, -1.0)
    }
    /// `self * c`.
    pub fn scale(&self, c: f64) -> Dual {
        self.chain(self.v * c, c)
    }
    /// `self + c`.
    pub fn offset(&self, c: f64) -> Dual {
        Dual { v: self.v + c, d: self.d.clone() }
    }
    /// `exp(self)`, continued linearly above [`EXP_LIMIT`].
    pub fn exp(&self) -> Dual {
        let (e, de) = limited_exp(self.v);
        self.chain(e, de)
    }
    /// `ln(self)` (NaN for values at or below zero).
    pub fn ln(&self) -> Dual {
        self.chain(self.v.ln(), 1.0 / self.v)
    }
    /// `sqrt(self)` (NaN below zero).
    pub fn sqrt(&self) -> Dual {
        let s = self.v.sqrt();
        self.chain(s, 0.5 / s)
    }
    /// `self ^ c` for a constant exponent (`self > 0`, or any sign for a whole `c`).
    pub fn powf(&self, c: f64) -> Dual {
        if c == 0.0 {
            return self.chain(1.0, 0.0);
        }
        if c.fract() == 0.0 && c.abs() < 1e9 {
            let k = c as i32;
            let p = self.v.powi(k);
            let dp = if k == 0 { 0.0 } else { c * self.v.powi(k - 1) };
            return self.chain(p, dp);
        }
        let p = self.v.powf(c);
        self.chain(p, c * self.v.powf(c - 1.0))
    }
    /// The larger of two, with the derivative of the one chosen.
    pub fn max(&self, other: &Dual) -> Dual {
        if self.v >= other.v { self.clone() } else { other.clone() }
    }
    /// The smaller of two.
    pub fn min(&self, other: &Dual) -> Dual {
        if self.v <= other.v { self.clone() } else { other.clone() }
    }
    /// True when the value and every derivative are finite.
    pub fn is_finite(&self) -> bool {
        self.v.is_finite() && self.d.iter().all(|x| x.is_finite())
    }
}

/// `exp(x)` and its derivative, continued linearly above [`EXP_LIMIT`].
pub fn limited_exp(x: f64) -> (f64, f64) {
    if x > EXP_LIMIT {
        let e = EXP_LIMIT.exp();
        (e * (1.0 + x - EXP_LIMIT), e)
    } else {
        let e = x.exp();
        (e, e)
    }
}

/// A unary operator of an [`Expr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-x`.
    Neg,
    /// `!x`: 1 when `x` is zero, else 0.
    Not,
}

/// A binary operator of an [`Expr`]. Comparisons and logic give 1 or 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `**` or `^`: `x^y`; NaN for a negative `x` with a fractional `y`.
    Pow,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `==`
    Eq,
    /// `!=` or `<>`
    Ne,
    /// `&&` or `&`
    And,
    /// `||` or `|`
    Or,
}

/// A built-in function of an [`Expr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    /// `abs(x)`
    Abs,
    /// `sgn(x)`: -1, 0 or 1.
    Sgn,
    /// `sqrt(x)`
    Sqrt,
    /// `exp(x)`
    Exp,
    /// `ln(x)` (also SPICE's `log(x)`).
    Ln,
    /// `log10(x)`
    Log10,
    /// `sin(x)`
    Sin,
    /// `cos(x)`
    Cos,
    /// `tan(x)`
    Tan,
    /// `asin(x)`
    Asin,
    /// `acos(x)`
    Acos,
    /// `atan(x)`
    Atan,
    /// `atan2(y, x)`
    Atan2,
    /// `sinh(x)`
    Sinh,
    /// `cosh(x)`
    Cosh,
    /// `tanh(x)`
    Tanh,
    /// `min(x, y)`
    Min,
    /// `max(x, y)`
    Max,
    /// `limit(x, a, b)`: `x` clamped between the smaller and the larger of `a` and `b`.
    Limit,
    /// `pwr(x, y)`: `|x|^y`.
    Pwr,
    /// `pwrs(x, y)`: `sgn(x) |x|^y`.
    Pwrs,
    /// `pow(x, y)`: as `x**y`.
    Pow,
    /// `u(x)`: 1 above zero, else 0.
    Step,
    /// `uramp(x)`: `max(x, 0)`.
    Uramp,
    /// `floor(x)`
    Floor,
    /// `ceil(x)`
    Ceil,
    /// `round(x)`
    Round,
    /// `int(x)`: towards zero.
    Int,
    /// `hypot(x, y)`
    Hypot,
}

impl Func {
    /// The number of arguments.
    pub fn arity(self) -> usize {
        match self {
            Func::Atan2 | Func::Min | Func::Max | Func::Pwr | Func::Pwrs | Func::Pow | Func::Hypot => 2,
            Func::Limit => 3,
            _ => 1,
        }
    }
}

/// A behavioural expression over the controls of a [`Law`] (`Var(k)` is
/// control `k`) and time.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A number.
    Const(f64),
    /// Control `k` of the law.
    Var(usize),
    /// The simulation time, s (0 in the operating point).
    Time,
    /// A unary operation.
    Unary(UnaryOp, Box<Expr>),
    /// A binary operation.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// A function call.
    Call(Func, Vec<Expr>),
    /// `if(c, a, b)` and `c ? a : b`: `a` where `c` is not zero.
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Piecewise linear through `(x, y)` points with increasing `x`, constant
    /// beyond the ends.
    Table(Box<Expr>, Vec<(f64, f64)>),
}

impl Expr {
    /// The value and derivatives at controls `u` and time `t`.
    pub fn eval(&self, u: &[f64], t: f64) -> Dual {
        let n = u.len();
        match self {
            Expr::Const(c) => Dual::constant(*c, n),
            Expr::Var(k) => Dual::variable(u.get(*k).copied().unwrap_or(f64::NAN), *k, n),
            Expr::Time => Dual::constant(t, n),
            Expr::Unary(UnaryOp::Neg, x) => x.eval(u, t).neg(),
            Expr::Unary(UnaryOp::Not, x) => Dual::constant(if x.eval(u, t).v == 0.0 { 1.0 } else { 0.0 }, n),
            Expr::Binary(op, a, b) => {
                let (x, y) = (a.eval(u, t), b.eval(u, t));
                let truth = |c: bool| Dual::constant(if c { 1.0 } else { 0.0 }, n);
                match op {
                    BinaryOp::Add => x.add(&y),
                    BinaryOp::Sub => x.sub(&y),
                    BinaryOp::Mul => x.mul(&y),
                    BinaryOp::Div => x.div(&y),
                    BinaryOp::Pow => pow(&x, &y),
                    BinaryOp::Lt => truth(x.v < y.v),
                    BinaryOp::Le => truth(x.v <= y.v),
                    BinaryOp::Gt => truth(x.v > y.v),
                    BinaryOp::Ge => truth(x.v >= y.v),
                    BinaryOp::Eq => truth(x.v == y.v),
                    BinaryOp::Ne => truth(x.v != y.v),
                    BinaryOp::And => truth(x.v != 0.0 && y.v != 0.0),
                    BinaryOp::Or => truth(x.v != 0.0 || y.v != 0.0),
                }
            }
            Expr::If(c, a, b) => {
                if c.eval(u, t).v != 0.0 {
                    a.eval(u, t)
                } else {
                    b.eval(u, t)
                }
            }
            Expr::Table(x, points) => {
                let x = x.eval(u, t);
                table(&x, points)
            }
            Expr::Call(f, args) => {
                let a: Vec<Dual> = args.iter().map(|e| e.eval(u, t)).collect();
                call(*f, &a, n)
            }
        }
    }

    /// True when the expression reads the time.
    pub fn uses_time(&self) -> bool {
        match self {
            Expr::Time => true,
            Expr::Const(_) | Expr::Var(_) => false,
            Expr::Unary(_, x) | Expr::Table(x, _) => x.uses_time(),
            Expr::Binary(_, a, b) => a.uses_time() || b.uses_time(),
            Expr::If(c, a, b) => c.uses_time() || a.uses_time() || b.uses_time(),
            Expr::Call(_, args) => args.iter().any(Expr::uses_time),
        }
    }

    /// The constant value when the expression reads no control and no time.
    pub fn constant(&self) -> Option<f64> {
        match self {
            Expr::Const(c) => Some(*c),
            Expr::Var(_) | Expr::Time => None,
            Expr::Unary(_, x) | Expr::Table(x, _) => x.constant().map(|_| self.eval(&[], 0.0).v),
            Expr::Binary(_, a, b) => (a.constant().is_some() && b.constant().is_some()).then(|| self.eval(&[], 0.0).v),
            Expr::If(c, a, b) => match c.constant() {
                Some(v) if v != 0.0 => a.constant(),
                Some(_) => b.constant(),
                None => None,
            },
            Expr::Call(_, args) => args.iter().all(|a| a.constant().is_some()).then(|| self.eval(&[], 0.0).v),
        }
    }

    /// `a op b`.
    pub fn binary(op: BinaryOp, a: Expr, b: Expr) -> Expr {
        Expr::Binary(op, Box::new(a), Box::new(b))
    }
}

fn pow(x: &Dual, y: &Dual) -> Dual {
    let constant_exponent = y.d.iter().all(|d| *d == 0.0);
    if constant_exponent {
        if x.v < 0.0 && y.v.fract() != 0.0 {
            return x.chain(f64::NAN, f64::NAN);
        }
        if x.v == 0.0 && y.v > 0.0 && y.v.fract() != 0.0 {
            return x.chain(0.0, if y.v > 1.0 { 0.0 } else { f64::INFINITY });
        }
        return x.powf(y.v);
    }
    // x^y = exp(y ln x) for x > 0.
    if x.v <= 0.0 {
        return x.chain(f64::NAN, f64::NAN);
    }
    y.mul(&x.ln()).exp()
}

fn table(x: &Dual, points: &[(f64, f64)]) -> Dual {
    let n = x.d.len();
    let Some(&(x0, y0)) = points.first() else { return Dual::constant(f64::NAN, n) };
    if x.v <= x0 {
        return Dual::constant(y0, n);
    }
    for w in points.windows(2) {
        let ((xa, ya), (xb, yb)) = (w[0], w[1]);
        if x.v <= xb {
            if xb == xa {
                return Dual::constant(yb, n);
            }
            let slope = (yb - ya) / (xb - xa);
            return x.chain(ya + slope * (x.v - xa), slope);
        }
    }
    Dual::constant(points.last().map_or(f64::NAN, |p| p.1), n)
}

fn call(f: Func, a: &[Dual], n: usize) -> Dual {
    let nan = || Dual::constant(f64::NAN, n);
    let x = match a.first() {
        Some(x) => x,
        None => return nan(),
    };
    match f {
        Func::Abs => {
            if x.v >= 0.0 {
                x.clone()
            } else {
                x.neg()
            }
        }
        Func::Sgn => Dual::constant(
            if x.v > 0.0 {
                1.0
            } else if x.v < 0.0 {
                -1.0
            } else {
                0.0
            },
            n,
        ),
        Func::Sqrt => x.sqrt(),
        Func::Exp => x.exp(),
        Func::Ln => x.ln(),
        Func::Log10 => x.chain(x.v.log10(), 1.0 / (x.v * std::f64::consts::LN_10)),
        Func::Sin => x.chain(x.v.sin(), x.v.cos()),
        Func::Cos => x.chain(x.v.cos(), -x.v.sin()),
        Func::Tan => {
            let c = x.v.cos();
            x.chain(x.v.tan(), 1.0 / (c * c))
        }
        Func::Asin => x.chain(x.v.asin(), 1.0 / (1.0 - x.v * x.v).sqrt()),
        Func::Acos => x.chain(x.v.acos(), -1.0 / (1.0 - x.v * x.v).sqrt()),
        Func::Atan => x.chain(x.v.atan(), 1.0 / (1.0 + x.v * x.v)),
        Func::Sinh => x.chain(x.v.sinh(), x.v.cosh()),
        Func::Cosh => x.chain(x.v.cosh(), x.v.sinh()),
        Func::Tanh => {
            let t = x.v.tanh();
            x.chain(t, 1.0 - t * t)
        }
        Func::Step => Dual::constant(if x.v > 0.0 { 1.0 } else { 0.0 }, n),
        Func::Uramp => {
            if x.v > 0.0 {
                x.clone()
            } else {
                Dual::constant(0.0, n)
            }
        }
        Func::Floor => Dual::constant(x.v.floor(), n),
        Func::Ceil => Dual::constant(x.v.ceil(), n),
        Func::Round => Dual::constant(x.v.round(), n),
        Func::Int => Dual::constant(x.v.trunc(), n),
        Func::Atan2 | Func::Min | Func::Max | Func::Pwr | Func::Pwrs | Func::Pow | Func::Hypot | Func::Limit => {
            let Some(y) = a.get(1) else { return nan() };
            match f {
                Func::Atan2 => {
                    // atan2(y = x, x = y): d/dy = x/(x²+y²), d/dx = -y/(x²+y²).
                    let r2 = x.v * x.v + y.v * y.v;
                    x.zip(y, x.v.atan2(y.v), y.v / r2, -x.v / r2)
                }
                Func::Min => x.min(y),
                Func::Max => x.max(y),
                Func::Pow => pow(x, y),
                Func::Pwr | Func::Pwrs => {
                    let magnitude = if x.v >= 0.0 { x.clone() } else { x.neg() };
                    let p = if magnitude.v == 0.0 && y.d.iter().all(|d| *d == 0.0) {
                        let value = if y.v == 0.0 { 1.0 } else { 0.0 };
                        let slope = if y.v == 1.0 { 1.0 } else { 0.0 };
                        magnitude.chain(value, slope)
                    } else {
                        pow(&magnitude, y)
                    };
                    if f == Func::Pwrs && x.v < 0.0 { p.neg() } else { p }
                }
                Func::Hypot => {
                    let h = x.v.hypot(y.v);
                    if h == 0.0 { Dual::constant(0.0, n) } else { x.zip(y, h, x.v / h, y.v / h) }
                }
                Func::Limit => {
                    let Some(z) = a.get(2) else { return nan() };
                    let (lo, hi) = if y.v <= z.v { (y, z) } else { (z, y) };
                    if x.v < lo.v {
                        lo.clone()
                    } else if x.v > hi.v {
                        hi.clone()
                    } else {
                        x.clone()
                    }
                }
                _ => unreachable!("two- and three-argument functions"),
            }
        }
    }
}

/// What controls a [`Law`]: a voltage difference or another element's current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// `V(p) - V(n)`.
    Voltage(NodeId, NodeId),
    /// The current of an element added before this one (`a -> b` convention).
    Current(ElementId),
}

/// A Newton step limit on one control. `sign` is the device polarity (+1
/// for NPN, N-channel; -1 for PNP, P-channel): the limit acts on
/// `sign * control`, the voltage in the device's own polarity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    /// None: linearised at the present iterate.
    None,
    /// A pn junction (SPICE `pnjlim`) with thermal voltage `vt` and critical voltage `vcrit`.
    Junction {
        /// `n V_T`, V.
        vt: f64,
        /// The critical voltage, V.
        vcrit: f64,
        /// Polarity.
        sign: f64,
    },
    /// A FET gate-source voltage (SPICE `fetlim`) around threshold `vto`.
    Fet {
        /// Threshold voltage, V, in the device's own polarity.
        vto: f64,
        /// Polarity.
        sign: f64,
    },
    /// A drain-source voltage (SPICE `limvds`).
    Vds {
        /// Polarity.
        sign: f64,
    },
    /// A control whose function changes steeply between `lo` and `hi` (a
    /// smooth switch's transition): a step into or across the window
    /// stops a quarter of the window past its edge, and steps inside it are
    /// at most a quarter of the window, so Newton walks through the steep
    /// part instead of jumping over it.
    Window {
        /// Lower edge.
        lo: f64,
        /// Upper edge.
        hi: f64,
    },
}

impl Limit {
    /// The value to linearise at for a new iterate `new`, given the last one `old`.
    pub fn apply(self, new: f64, old: f64) -> f64 {
        match self {
            Limit::None => new,
            Limit::Junction { vt, vcrit, sign } => sign * pnjlim(sign * new, sign * old, vt, vcrit),
            Limit::Fet { vto, sign } => sign * fetlim(sign * new, sign * old, vto),
            Limit::Vds { sign } => sign * limvds(sign * new, sign * old),
            Limit::Window { lo, hi } => window_limit(new, old, lo, hi),
        }
    }
}

/// The [`Limit::Window`] step limit.
pub fn window_limit(new: f64, old: f64, lo: f64, hi: f64) -> f64 {
    let quarter = (hi - lo) / 4.0;
    if old < lo {
        if new > lo + quarter { lo + quarter } else { new }
    } else if old > hi {
        if new < hi - quarter { hi - quarter } else { new }
    } else {
        new.clamp(old - quarter, old + quarter)
    }
}

/// SPICE's `pnjlim`.
pub fn pnjlim(v_new: f64, v_old: f64, vt: f64, vcrit: f64) -> f64 {
    if v_new > vcrit && (v_new - v_old).abs() > 2.0 * vt {
        if v_old > 0.0 {
            let arg = 1.0 + (v_new - v_old) / vt;
            if arg > 0.0 { v_old + vt * arg.ln() } else { vcrit }
        } else {
            vt * (v_new / vt).ln()
        }
    } else {
        v_new
    }
}

/// SPICE3's `DEVfetlim`.
pub fn fetlim(v_new: f64, v_old: f64, vto: f64) -> f64 {
    let vtsthi = (2.0 * (v_old - vto)).abs() + 2.0;
    let vtstlo = vtsthi / 2.0 + 2.0;
    let vtox = vto + 3.5;
    let delv = v_new - v_old;
    let mut v = v_new;
    if v_old >= vto {
        if v_old >= vtox {
            if delv <= 0.0 {
                if v_new >= vtox {
                    if -delv > vtstlo {
                        v = v_old - vtstlo;
                    }
                } else {
                    v = v.max(vto + 2.0);
                }
            } else if delv >= vtsthi {
                v = v_old + vtsthi;
            }
        } else if delv <= 0.0 {
            v = v.max(vto - 0.5);
        } else {
            v = v.min(vto + 4.0);
        }
    } else if delv <= 0.0 {
        if -delv > vtsthi {
            v = v_old - vtsthi;
        }
    } else {
        let vtemp = vto + 0.5;
        if v_new <= vtemp {
            if delv > vtstlo {
                v = v_old + vtstlo;
            }
        } else {
            v = vtemp;
        }
    }
    v
}

/// SPICE3's `DEVlimvds`.
pub fn limvds(v_new: f64, v_old: f64) -> f64 {
    if v_old >= 3.5 {
        if v_new > v_old {
            v_new.min(3.0 * v_old + 2.0)
        } else if v_new < 3.5 {
            v_new.max(2.0)
        } else {
            v_new
        }
    } else if v_new > v_old {
        v_new.min(4.0)
    } else {
        v_new.max(-0.5)
    }
}

/// Whether a law sets a current or a voltage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// A current from `a` through the element to `b`.
    Current,
    /// `V(a) - V(b)`; the current is a branch unknown.
    Voltage,
}

/// The noise of a law element at its operating point (current outputs only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LawNoise {
    /// Shot noise `2 q |I|` of the element's current.
    Shot,
    /// Flicker noise `KF |I|^AF / (f * divisor)` of the element's current.
    Flicker {
        /// `KF`.
        kf: f64,
        /// `AF`.
        af: f64,
        /// Divisor of the density (`C_ox L_eff²` for a MOSFET, 1 otherwise).
        divisor: f64,
    },
    /// Thermal noise `4 k T g` of a nonlinear resistance, `g` the current's
    /// derivative with respect to this control (its own voltage).
    Conductance {
        /// Index of the control across the element.
        control: usize,
    },
    /// Thermal noise of a FET channel, `4 k T (2/3) g_m`, with `g_m` the sum
    /// of the current's derivatives with respect to these controls.
    Channel {
        /// Indices of the gate controls.
        gate_controls: [usize; 2],
        /// How many of `gate_controls` count.
        count: usize,
    },
}

/// The function of a law.
#[derive(Debug, Clone, PartialEq)]
pub enum Function {
    /// A behavioural expression.
    Expr {
        /// `f(u, t)`.
        value: Arc<Expr>,
    },
    /// A branch of a semiconductor device.
    Device(Arc<crate::semiconductors::DeviceBranch>),
}

/// A nonlinear element's law: output, controls with their limits, function and noise.
#[derive(Debug, Clone, PartialEq)]
pub struct Law {
    /// Current or voltage output.
    pub output: Output,
    /// The controls; [`Expr::Var`] indices refer to them.
    pub controls: Vec<Control>,
    /// One limit per control.
    pub limits: Vec<Limit>,
    /// The function.
    pub function: Function,
    /// Noise generators (current outputs only).
    pub noise: Vec<LawNoise>,
}

impl Law {
    /// A behavioural law without limits or noise.
    pub fn expression(output: Output, controls: Vec<Control>, value: Expr) -> Law {
        let limits = vec![Limit::None; controls.len()];
        Law { output, controls, limits, function: Function::Expr { value: Arc::new(value) }, noise: Vec::new() }
    }

    /// `f(u, t)` and its derivatives.
    pub fn value(&self, u: &[f64], t: f64) -> Dual {
        match &self.function {
            Function::Expr { value } => value.eval(u, t),
            Function::Device(branch) => branch.current(u),
        }
    }

    /// The charge `q(u)` and its derivatives, for elements that store charge.
    pub fn charge(&self, u: &[f64]) -> Option<Dual> {
        match &self.function {
            Function::Expr { .. } => None,
            Function::Device(branch) => branch.charge(u),
        }
    }

    /// Whether the element stores charge.
    pub fn has_charge(&self) -> bool {
        match &self.function {
            Function::Expr { .. } => false,
            Function::Device(branch) => branch.has_charge(),
        }
    }

    /// Whether the function reads the simulation time.
    pub fn uses_time(&self) -> bool {
        match &self.function {
            Function::Expr { value } => value.uses_time(),
            Function::Device(_) => false,
        }
    }

    /// Check the law's shape (one limit per control, variables in range).
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.limits.len() != self.controls.len() {
            return Err(format!("{} limits for {} controls", self.limits.len(), self.controls.len()));
        }
        if let Function::Expr { value } = &self.function {
            let mut highest = None;
            visit_vars(value, &mut |k| highest = Some(highest.map_or(k, |h: usize| h.max(k))));
            if let Some(k) = highest
                && k >= self.controls.len()
            {
                return Err(format!("expression reads control {k} of {}", self.controls.len()));
            }
        }
        if self.output == Output::Voltage && !self.noise.is_empty() {
            return Err("a voltage law has no noise generators".to_string());
        }
        Ok(())
    }
}

fn visit_vars(e: &Expr, f: &mut impl FnMut(usize)) {
    match e {
        Expr::Var(k) => f(*k),
        Expr::Const(_) | Expr::Time => {}
        Expr::Unary(_, x) | Expr::Table(x, _) => visit_vars(x, f),
        Expr::Binary(_, a, b) => {
            visit_vars(a, f);
            visit_vars(b, f);
        }
        Expr::If(c, a, b) => {
            visit_vars(c, f);
            visit_vars(a, f);
            visit_vars(b, f);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| visit_vars(a, f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numeric(e: &Expr, u: &[f64], k: usize) -> f64 {
        let h = 1e-6 * u[k].abs().max(1.0);
        let (mut a, mut b) = (u.to_vec(), u.to_vec());
        a[k] += h;
        b[k] -= h;
        (e.eval(&a, 0.0).v - e.eval(&b, 0.0).v) / (2.0 * h)
    }

    #[test]
    fn derivatives_match_finite_differences() {
        use BinaryOp::*;
        let v = |k| Box::new(Expr::Var(k));
        let exprs = [
            Expr::Call(Func::Tanh, vec![Expr::binary(Mul, Expr::Const(3.0), Expr::Var(0))]),
            Expr::binary(Div, Expr::Var(0), Expr::binary(Add, Expr::Var(1), Expr::Const(2.0))),
            Expr::Binary(Pow, v(1), Box::new(Expr::Const(2.5))),
            Expr::Call(Func::Pwrs, vec![Expr::Var(0), Expr::Const(1.5)]),
            Expr::Call(
                Func::Limit,
                vec![Expr::binary(Mul, Expr::Var(0), Expr::Const(10.0)), Expr::Const(-1.0), Expr::Const(1.0)],
            ),
            Expr::Table(v(0), vec![(-1.0, 0.0), (0.0, 1.0), (2.0, 5.0)]),
            Expr::Call(Func::Exp, vec![Expr::Var(1)]),
            Expr::Binary(Pow, v(1), v(0)),
            Expr::Call(Func::Atan2, vec![Expr::Var(0), Expr::Var(1)]),
        ];
        let u = [0.03, 1.7];
        for e in &exprs {
            let d = e.eval(&u, 0.0);
            for k in 0..2 {
                let n = numeric(e, &u, k);
                assert!((d.d[k] - n).abs() <= 1e-6 * n.abs().max(1.0), "{e:?} d/du{k}: {} vs {n}", d.d[k]);
            }
        }
    }

    #[test]
    fn piecewise_semantics() {
        let limit = Expr::Call(Func::Limit, vec![Expr::Var(0), Expr::Const(2.0), Expr::Const(-2.0)]);
        assert_eq!(limit.eval(&[5.0], 0.0).v, 2.0);
        assert_eq!(limit.eval(&[-5.0], 0.0).v, -2.0);
        assert_eq!(limit.eval(&[0.5], 0.0).d, vec![1.0]);
        let table = Expr::Table(Box::new(Expr::Var(0)), vec![(0.0, 0.0), (1.0, 2.0)]);
        assert_eq!(table.eval(&[-1.0], 0.0).v, 0.0);
        assert_eq!(table.eval(&[0.25], 0.0).v, 0.5);
        assert_eq!(table.eval(&[9.0], 0.0).v, 2.0);
        let choose = Expr::If(
            Box::new(Expr::binary(BinaryOp::Gt, Expr::Var(0), Expr::Const(0.0))),
            Box::new(Expr::Const(1.0)),
            Box::new(Expr::Var(0)),
        );
        assert_eq!(choose.eval(&[1.0], 0.0).v, 1.0);
        assert_eq!(choose.eval(&[-3.0], 0.0).v, -3.0);
        assert!(Expr::binary(BinaryOp::Pow, Expr::Const(-2.0), Expr::Const(0.5)).eval(&[], 0.0).v.is_nan());
        assert_eq!(Expr::binary(BinaryOp::Pow, Expr::Const(-2.0), Expr::Const(3.0)).eval(&[], 0.0).v, -8.0);
        assert_eq!(Expr::binary(BinaryOp::Add, Expr::Const(1.0), Expr::Const(2.0)).constant(), Some(3.0));
        assert_eq!(Expr::Var(0).constant(), None);
    }

    #[test]
    fn limiters() {
        assert_eq!(pnjlim(0.5, 0.4, 0.025, 0.7), 0.5);
        let v = pnjlim(5.0, 0.7, 0.025, 0.6);
        assert!(v > 0.7 && v < 0.9, "{v}");
        assert_eq!(fetlim(1.0, 1.0, 0.5), 1.0);
        assert!(fetlim(20.0, 1.0, 0.5) < 20.0);
        assert_eq!(limvds(1.0, 0.5), 1.0);
        assert_eq!(limvds(10.0, 0.5), 4.0);
    }
}
