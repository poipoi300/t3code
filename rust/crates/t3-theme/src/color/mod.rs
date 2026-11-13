//! CSS colour model: parsing, conversion between colour spaces, gamut mapping,
//! `color-mix()` and the canonical OKLCH serialisation the theme files use.
//!
//! Conversions follow CSS Color 4 (matrices and transfer functions as written in
//! the specification). Gamut mapping follows the CSS Color 4 chroma-reduction
//! algorithm, which is what Chrome applies when an out-of-gamut colour is
//! painted into an sRGB surface; the tests compare 8-bit results with Chrome.

mod named;

use serde::{Deserialize, Serialize};

pub type Vec3 = [f64; 3];
type Mat3 = [[f64; 3]; 3];

const fn mul(m: &Mat3, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

const fn inv(m: Mat3) -> Mat3 {
    let a = m[0][0];
    let b = m[0][1];
    let c = m[0][2];
    let d = m[1][0];
    let e = m[1][1];
    let f = m[1][2];
    let g = m[2][0];
    let h = m[2][1];
    let i = m[2][2];
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let s = 1.0 / det;
    [
        [
            (e * i - f * h) * s,
            (c * h - b * i) * s,
            (b * f - c * e) * s,
        ],
        [
            (f * g - d * i) * s,
            (a * i - c * g) * s,
            (c * d - a * f) * s,
        ],
        [
            (d * h - e * g) * s,
            (b * g - a * h) * s,
            (a * e - b * d) * s,
        ],
    ]
}

const LIN_SRGB_TO_XYZ: Mat3 = [
    [506752.0 / 1228815.0, 87881.0 / 245763.0, 12673.0 / 70218.0],
    [87098.0 / 409605.0, 175762.0 / 245763.0, 12673.0 / 175545.0],
    [7918.0 / 409605.0, 87881.0 / 737289.0, 1001167.0 / 1053270.0],
];
const XYZ_TO_LIN_SRGB: Mat3 = inv(LIN_SRGB_TO_XYZ);

const LIN_P3_TO_XYZ: Mat3 = [
    [
        608311.0 / 1250200.0,
        189793.0 / 714400.0,
        198249.0 / 1000160.0,
    ],
    [
        35783.0 / 156275.0,
        247089.0 / 357200.0,
        198249.0 / 2500400.0,
    ],
    [0.0, 32229.0 / 714400.0, 5220557.0 / 5000800.0],
];
const XYZ_TO_LIN_P3: Mat3 = inv(LIN_P3_TO_XYZ);

const LIN_A98_TO_XYZ: Mat3 = [
    [
        573536.0 / 994567.0,
        263643.0 / 1420810.0,
        187206.0 / 994567.0,
    ],
    [
        591459.0 / 1989134.0,
        6239551.0 / 9945670.0,
        374412.0 / 4972835.0,
    ],
    [
        53769.0 / 1989134.0,
        351524.0 / 4972835.0,
        4929758.0 / 4972835.0,
    ],
];
const XYZ_TO_LIN_A98: Mat3 = inv(LIN_A98_TO_XYZ);

const LIN_REC2020_TO_XYZ: Mat3 = [
    [
        63426534.0 / 99577255.0,
        20160776.0 / 139408157.0,
        47086771.0 / 278816314.0,
    ],
    [
        26158966.0 / 99577255.0,
        472592308.0 / 697040785.0,
        8267143.0 / 139408157.0,
    ],
    [0.0, 19567812.0 / 697040785.0, 295819943.0 / 278816314.0],
];
const XYZ_TO_LIN_REC2020: Mat3 = inv(LIN_REC2020_TO_XYZ);

const LIN_PROPHOTO_TO_XYZ50: Mat3 = [
    [0.7977666449006423, 0.13518129740053308, 0.0313477341283922],
    [0.2880748288194013, 0.711835234241873, 0.00008993693872564],
    [0.0, 0.0, 0.8251046025104602],
];
const XYZ50_TO_LIN_PROPHOTO: Mat3 = inv(LIN_PROPHOTO_TO_XYZ50);

const D50_TO_D65: Mat3 = [
    [0.955473421488075, -0.02309845494876471, 0.06325924320057072],
    [
        -0.0283697093338637,
        1.0099953980813041,
        0.021041441191917323,
    ],
    [
        0.012314014864481998,
        -0.020507649298898964,
        1.330365926242124,
    ],
];
const D65_TO_D50: Mat3 = inv(D50_TO_D65);

const XYZ_TO_LMS: Mat3 = [
    [0.819022437996703, 0.3619062600528904, -0.1288737815209879],
    [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
    [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
];
const LMS_TO_XYZ: Mat3 = inv(XYZ_TO_LMS);
const LMS_TO_OKLAB: Mat3 = [
    [0.210454268309314, 0.7936177747023054, -0.0040720430116193],
    [1.9779985324311684, -2.42859224204858, 0.450593709617411],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
const OKLAB_TO_LMS: Mat3 = inv(LMS_TO_OKLAB);

const CULORI_XYZ50_TO_LRGB: Mat3 = [
    [3.1341359569958707, -1.6173863321612538, -0.4906619460083532],
    [-0.978795502912089, 1.916254567259524, 0.03344273116131949],
    [0.07195537988411677, -0.2289768264158322, 1.405386058324125],
];
const CULORI_LRGB_TO_OKLAB_LMS: Mat3 = [
    [0.412221469470763, 0.5363325372617348, 0.0514459932675022],
    [0.2119034958178252, 0.6806995506452344, 0.1073969535369406],
    [0.0883024591900564, 0.2817188391361215, 0.6299787016738222],
];
const CULORI_LMS_TO_OKLAB: Mat3 = [
    [0.210454268309314, 0.7936177747023054, -0.0040720430116193],
    [1.9779985324311684, -2.428_592_242_048_58, 0.450593709617411],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
const CULORI_OKLAB_TO_LMS: Mat3 = [
    [1.0, 0.3963377773761749, 0.2158037573099136],
    [1.0, -0.1055613458156586, -0.0638541728258133],
    [1.0, -0.0894841775298119, -1.2914855480194092],
];
const CULORI_LMS_TO_LRGB: Mat3 = [
    [
        4.076_741_636_075_957,
        -3.3077115392580616,
        0.2309699031821044,
    ],
    [-1.2684379732850317, 2.6097573492876887, -0.3413193760026573],
    [-0.0041960761386756, -0.7034186179359362, 1.7076146940746117],
];

const D50: Vec3 = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
const LAB_E: f64 = 216.0 / 24389.0;
const LAB_K: f64 = 24389.0 / 27.0;

fn srgb_decode(v: f64) -> f64 {
    let a = v.abs();
    if a <= 0.04045 {
        v / 12.92
    } else {
        v.signum() * ((a + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(v: f64) -> f64 {
    let a = v.abs();
    if a <= 0.0031308 {
        v * 12.92
    } else {
        v.signum() * (1.055 * a.powf(1.0 / 2.4) - 0.055)
    }
}

fn a98_decode(v: f64) -> f64 {
    v.signum() * v.abs().powf(563.0 / 256.0)
}

fn a98_encode(v: f64) -> f64 {
    v.signum() * v.abs().powf(256.0 / 563.0)
}

fn prophoto_decode(v: f64) -> f64 {
    let a = v.abs();
    if a <= 16.0 / 512.0 {
        v / 16.0
    } else {
        v.signum() * a.powf(1.8)
    }
}

fn prophoto_encode(v: f64) -> f64 {
    let a = v.abs();
    if a >= 1.0 / 512.0 {
        v.signum() * a.powf(1.0 / 1.8)
    } else {
        16.0 * v
    }
}

const REC2020_ALPHA: f64 = 1.09929682680944;
const REC2020_BETA: f64 = 0.018053968510807;

fn rec2020_decode(v: f64) -> f64 {
    let a = v.abs();
    if a < REC2020_BETA * 4.5 {
        v / 4.5
    } else {
        v.signum() * ((a + REC2020_ALPHA - 1.0) / REC2020_ALPHA).powf(1.0 / 0.45)
    }
}

fn rec2020_encode(v: f64) -> f64 {
    let a = v.abs();
    if a >= REC2020_BETA {
        v.signum() * (REC2020_ALPHA * a.powf(0.45) - (REC2020_ALPHA - 1.0))
    } else {
        4.5 * v
    }
}

fn map3(v: Vec3, f: fn(f64) -> f64) -> Vec3 {
    [f(v[0]), f(v[1]), f(v[2])]
}

/// A CSS colour space this crate can convert between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Space {
    Srgb,
    SrgbLinear,
    DisplayP3,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    XyzD50,
    XyzD65,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Hsl,
    Hwb,
}

/// A colour as written in CSS: its space, three components and alpha.
///
/// RGB spaces hold 0..1 gamma-encoded components, polar spaces hold hue in
/// degrees. Components are not clamped, so wide-gamut and out-of-gamut values
/// survive a round trip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub space: Space,
    pub c: Vec3,
    #[serde(default = "one")]
    pub alpha: f64,
}

fn one() -> f64 {
    1.0
}

/// The hue of an OKLCH colour counts as missing below this chroma.
const POWERLESS_OKLCH_CHROMA: f64 = 0.000004;

impl Color {
    pub const fn new(space: Space, c: Vec3, alpha: f64) -> Self {
        Self { space, c, alpha }
    }

    pub const fn srgb(r: f64, g: f64, b: f64) -> Self {
        Self::new(Space::Srgb, [r, g, b], 1.0)
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self::srgb(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
        )
    }

    pub const fn oklch(l: f64, c: f64, h: f64) -> Self {
        Self::new(Space::Oklch, [l, c, h], 1.0)
    }

    pub const fn with_alpha(mut self, alpha: f64) -> Self {
        self.alpha = alpha;
        self
    }

    pub const TRANSPARENT: Color = Color::new(Space::Srgb, [0.0, 0.0, 0.0], 0.0);

    /// Parses a literal CSS colour. Accepts hex, named colours, `transparent`,
    /// `rgb()`, `hsl()`, `hwb()`, `lab()`, `lch()`, `oklab()`, `oklch()` and
    /// `color()`. Returns `None` for anything else, including `var()`,
    /// `color-mix()` and relative colours.
    pub fn parse(input: &str) -> Option<Color> {
        let text = input.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(hex) = text.strip_prefix('#') {
            return parse_hex(hex);
        }
        let lower = text.to_ascii_lowercase();
        if lower == "transparent" {
            return Some(Color::TRANSPARENT);
        }
        if let Some(open) = lower.find('(') {
            if !lower.ends_with(')') {
                return None;
            }
            let name = lower[..open].trim();
            let body = &lower[open + 1..lower.len() - 1];
            return parse_function(name, body);
        }
        named::NAMED
            .binary_search_by(|(n, _)| (*n).cmp(lower.as_str()))
            .ok()
            .map(|index| {
                let value = named::NAMED[index].1;
                Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
            })
    }

    /// The colour in CIE XYZ with the D65 white point.
    pub fn to_xyz_d65(&self) -> Vec3 {
        let c = self.c;
        match self.space {
            Space::XyzD65 => c,
            Space::XyzD50 => mul(&D50_TO_D65, c),
            Space::Srgb => mul(&LIN_SRGB_TO_XYZ, map3(c, srgb_decode)),
            Space::SrgbLinear => mul(&LIN_SRGB_TO_XYZ, c),
            Space::DisplayP3 => mul(&LIN_P3_TO_XYZ, map3(c, srgb_decode)),
            Space::A98Rgb => mul(&LIN_A98_TO_XYZ, map3(c, a98_decode)),
            Space::Rec2020 => mul(&LIN_REC2020_TO_XYZ, map3(c, rec2020_decode)),
            Space::ProphotoRgb => mul(
                &D50_TO_D65,
                mul(&LIN_PROPHOTO_TO_XYZ50, map3(c, prophoto_decode)),
            ),
            Space::Hsl => Color::new(Space::Srgb, hsl_to_srgb(c), self.alpha).to_xyz_d65(),
            Space::Hwb => Color::new(Space::Srgb, hwb_to_srgb(c), self.alpha).to_xyz_d65(),
            Space::Lab => mul(&D50_TO_D65, lab_to_xyz_d50(c)),
            Space::Lch => mul(&D50_TO_D65, lab_to_xyz_d50(polar_to_rect(c))),
            Space::Oklab => oklab_to_xyz(c),
            Space::Oklch => oklab_to_xyz(polar_to_rect(c)),
        }
    }

    pub fn from_xyz_d65(xyz: Vec3, space: Space, alpha: f64) -> Color {
        let c = match space {
            Space::XyzD65 => xyz,
            Space::XyzD50 => mul(&D65_TO_D50, xyz),
            Space::Srgb => map3(mul(&XYZ_TO_LIN_SRGB, xyz), srgb_encode),
            Space::SrgbLinear => mul(&XYZ_TO_LIN_SRGB, xyz),
            Space::DisplayP3 => map3(mul(&XYZ_TO_LIN_P3, xyz), srgb_encode),
            Space::A98Rgb => map3(mul(&XYZ_TO_LIN_A98, xyz), a98_encode),
            Space::Rec2020 => map3(mul(&XYZ_TO_LIN_REC2020, xyz), rec2020_encode),
            Space::ProphotoRgb => map3(
                mul(&XYZ50_TO_LIN_PROPHOTO, mul(&D65_TO_D50, xyz)),
                prophoto_encode,
            ),
            Space::Hsl => srgb_to_hsl(map3(mul(&XYZ_TO_LIN_SRGB, xyz), srgb_encode)),
            Space::Hwb => srgb_to_hwb(map3(mul(&XYZ_TO_LIN_SRGB, xyz), srgb_encode)),
            Space::Lab => xyz_d50_to_lab(mul(&D65_TO_D50, xyz)),
            Space::Lch => rect_to_polar(xyz_d50_to_lab(mul(&D65_TO_D50, xyz)), 0.0),
            Space::Oklab => xyz_to_oklab(xyz),
            Space::Oklch => rect_to_polar(xyz_to_oklab(xyz), POWERLESS_OKLCH_CHROMA),
        };
        Color { space, c, alpha }
    }

    /// Converts to another space. Conversions between OKLab and the sRGB family
    /// use the direct matrices culori uses (the theme code's reference
    /// implementation) instead of a detour through XYZ, so canonical OKLCH
    /// strings agree to the last printed digit.
    pub fn to_space(&self, space: Space) -> Color {
        if self.space == space {
            return *self;
        }
        if matches!(
            self.space,
            Space::Lab | Space::Lch | Space::XyzD50 | Space::ProphotoRgb
        ) && matches!(
            space,
            Space::Srgb | Space::SrgbLinear | Space::Oklab | Space::Oklch
        ) {
            // culori reaches sRGB from D50 through its own Bradford-adapted matrix.
            let xyz50 = match self.space {
                Space::XyzD50 => self.c,
                Space::Lab => lab_to_xyz_d50(self.c),
                Space::ProphotoRgb => mul(&LIN_PROPHOTO_TO_XYZ50, map3(self.c, prophoto_decode)),
                _ => lab_to_xyz_d50(polar_to_rect(self.c)),
            };
            let linear = Color::new(
                Space::SrgbLinear,
                mul(&CULORI_XYZ50_TO_LRGB, xyz50),
                self.alpha,
            );
            return linear.to_space(space);
        }
        let ok = |s: Space| matches!(s, Space::Oklab | Space::Oklch);
        let rgb = |s: Space| matches!(s, Space::Srgb | Space::SrgbLinear | Space::Hsl | Space::Hwb);
        if ok(space) && !ok(self.space) {
            let srgb = if rgb(self.space) {
                *self
            } else {
                self.to_space(Space::Srgb)
            };
            let linear = match srgb.space {
                Space::SrgbLinear => srgb.c,
                _ => map3(
                    Color::new(Space::Srgb, srgb.rgb_components(), 1.0).c,
                    srgb_decode,
                ),
            };
            let lab = mul(&CULORI_LRGB_TO_OKLAB_LMS, linear);
            let lab = mul(
                &CULORI_LMS_TO_OKLAB,
                [lab[0].cbrt(), lab[1].cbrt(), lab[2].cbrt()],
            );
            let c = if space == Space::Oklab {
                lab
            } else {
                rect_to_polar(lab, 0.0)
            };
            return Color {
                space,
                c,
                alpha: self.alpha,
            };
        }
        if ok(self.space) && !ok(space) && (rgb(space) || space == Space::Srgb) {
            let lab = if self.space == Space::Oklab {
                self.c
            } else {
                polar_to_rect(self.c)
            };
            let lms = mul(&CULORI_OKLAB_TO_LMS, lab);
            let linear = mul(
                &CULORI_LMS_TO_LRGB,
                [lms[0].powi(3), lms[1].powi(3), lms[2].powi(3)],
            );
            let linear_color = Color::new(Space::SrgbLinear, linear, self.alpha);
            return linear_color.to_space(space);
        }
        Color::from_xyz_d65(self.to_xyz_d65(), space, self.alpha)
    }

    fn rgb_components(&self) -> Vec3 {
        match self.space {
            Space::Hsl => hsl_to_srgb(self.c),
            Space::Hwb => hwb_to_srgb(self.c),
            _ => self.c,
        }
    }

    /// `[l, c, h]` with hue in degrees, `0` for an achromatic colour.
    pub fn to_oklch(&self) -> Vec3 {
        self.to_space(Space::Oklch).c
    }

    pub fn to_oklab(&self) -> Vec3 {
        self.to_space(Space::Oklab).c
    }

    /// Gamma-encoded sRGB components without clamping.
    pub fn to_srgb_unclamped(&self) -> Vec3 {
        self.to_space(Space::Srgb).c
    }

    /// Gamma-encoded sRGB components after the CSS Color 4 gamut mapping.
    pub fn to_srgb_gamut_mapped(&self) -> Vec3 {
        let oklch = self.to_space(Space::Oklch).c;
        map_oklch_to_srgb(oklch)
    }

    /// Gamma-encoded sRGB components clipped to `0..=1` per channel, which is how
    /// Chrome paints an out-of-gamut colour onto an sRGB surface.
    pub fn to_srgb_clipped(&self) -> Vec3 {
        clip01(self.to_srgb_unclamped())
    }

    /// 8-bit sRGB and alpha the way Chrome paints this colour.
    pub fn to_rgba8(&self) -> [u8; 4] {
        let rgb = self.to_srgb_clipped();
        [
            channel_to_u8(rgb[0]),
            channel_to_u8(rgb[1]),
            channel_to_u8(rgb[2]),
            channel_to_u8(self.alpha),
        ]
    }

    /// `#rrggbb`, or `#rrggbbaa` when translucent.
    pub fn to_hex(&self) -> String {
        let [r, g, b, a] = self.to_rgba8();
        if self.alpha >= 1.0 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    pub fn to_css(&self) -> String {
        let [a, b, c] = self.c;
        let alpha = if self.alpha < 1.0 {
            format!(" / {}", trim_number(self.alpha, 4))
        } else {
            String::new()
        };
        let body = |name: &str| {
            format!(
                "{name}({} {} {}{alpha})",
                trim_number(a, 6),
                trim_number(b, 6),
                trim_number(c, 6)
            )
        };
        match self.space {
            Space::Oklch => body("oklch"),
            Space::Oklab => body("oklab"),
            Space::Lab => body("lab"),
            Space::Lch => body("lch"),
            Space::Hsl => format!(
                "hsl({} {}% {}%{alpha})",
                trim_number(a, 3),
                trim_number(b * 100.0, 3),
                trim_number(c * 100.0, 3)
            ),
            Space::Hwb => format!(
                "hwb({} {}% {}%{alpha})",
                trim_number(a, 3),
                trim_number(b * 100.0, 3),
                trim_number(c * 100.0, 3)
            ),
            other => format!(
                "color({} {} {} {}{alpha})",
                color_function_name(other),
                trim_number(a, 6),
                trim_number(b, 6),
                trim_number(c, 6)
            ),
        }
    }
}

fn color_function_name(space: Space) -> &'static str {
    match space {
        Space::Srgb => "srgb",
        Space::SrgbLinear => "srgb-linear",
        Space::DisplayP3 => "display-p3",
        Space::A98Rgb => "a98-rgb",
        Space::ProphotoRgb => "prophoto-rgb",
        Space::Rec2020 => "rec2020",
        Space::XyzD50 => "xyz-d50",
        _ => "xyz-d65",
    }
}

fn channel_to_u8(value: f64) -> u8 {
    // Browsers round half up on the 0..255 scale.
    (value.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

fn polar_to_rect(c: Vec3) -> Vec3 {
    let hue = c[2].to_radians();
    [c[0], c[1] * hue.cos(), c[1] * hue.sin()]
}

fn rect_to_polar(c: Vec3, powerless_below: f64) -> Vec3 {
    let chroma = c[1].hypot(c[2]);
    let hue = if chroma <= powerless_below || chroma == 0.0 {
        0.0
    } else {
        normalize_hue(c[2].atan2(c[1]).to_degrees())
    };
    [c[0], chroma, hue]
}

pub fn normalize_hue(hue: f64) -> f64 {
    let wrapped = hue % 360.0;
    if wrapped < 0.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

fn lab_to_xyz_d50(c: Vec3) -> Vec3 {
    let fy = (c[0] + 16.0) / 116.0;
    let fx = c[1] / 500.0 + fy;
    let fz = fy - c[2] / 200.0;
    let f = |v: f64| {
        if v.powi(3) > LAB_E {
            v.powi(3)
        } else {
            (116.0 * v - 16.0) / LAB_K
        }
    };
    let y = if c[0] > LAB_K * LAB_E {
        fy.powi(3)
    } else {
        c[0] / LAB_K
    };
    [f(fx) * D50[0], y * D50[1], f(fz) * D50[2]]
}

fn xyz_d50_to_lab(xyz: Vec3) -> Vec3 {
    let f = |v: f64| {
        if v > LAB_E {
            v.cbrt()
        } else {
            (LAB_K * v + 16.0) / 116.0
        }
    };
    let fx = f(xyz[0] / D50[0]);
    let fy = f(xyz[1] / D50[1]);
    let fz = f(xyz[2] / D50[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn oklab_to_xyz(lab: Vec3) -> Vec3 {
    let lms = mul(&OKLAB_TO_LMS, lab);
    mul(
        &LMS_TO_XYZ,
        [lms[0].powi(3), lms[1].powi(3), lms[2].powi(3)],
    )
}

fn xyz_to_oklab(xyz: Vec3) -> Vec3 {
    let lms = mul(&XYZ_TO_LMS, xyz);
    mul(&LMS_TO_OKLAB, [lms[0].cbrt(), lms[1].cbrt(), lms[2].cbrt()])
}

fn hsl_to_srgb(c: Vec3) -> Vec3 {
    let h = normalize_hue(c[0]);
    let (s, l) = (c[1], c[2]);
    let f = |n: f64| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn srgb_to_hsl(c: Vec3) -> Vec3 {
    let [r, g, b] = c;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return [0.0, 0.0, l];
    }
    let s = if l == 0.0 || l == 1.0 {
        0.0
    } else {
        (max - l) / l.min(1.0 - l)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h * 60.0, s, l]
}

fn hwb_to_srgb(c: Vec3) -> Vec3 {
    let (w, b) = (c[1], c[2]);
    if w + b >= 1.0 {
        let gray = w / (w + b);
        return [gray, gray, gray];
    }
    let rgb = hsl_to_srgb([c[0], 1.0, 0.5]);
    [
        rgb[0] * (1.0 - w - b) + w,
        rgb[1] * (1.0 - w - b) + w,
        rgb[2] * (1.0 - w - b) + w,
    ]
}

fn srgb_to_hwb(c: Vec3) -> Vec3 {
    let hsl = srgb_to_hsl(c);
    let w = c[0].min(c[1]).min(c[2]);
    let b = 1.0 - c[0].max(c[1]).max(c[2]);
    [hsl[0], w, b]
}

// ---------------------------------------------------------------- gamut mapping

const JND: f64 = 0.02;
const GAMUT_EPSILON: f64 = 0.0001;

fn oklch_to_linear_srgb(oklch: Vec3) -> Vec3 {
    mul(&XYZ_TO_LIN_SRGB, oklab_to_xyz(polar_to_rect(oklch)))
}

fn in_srgb_gamut(linear: Vec3) -> bool {
    linear.iter().all(|c| (0.0..=1.0).contains(c))
}

fn clip01(c: Vec3) -> Vec3 {
    [
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
    ]
}

fn delta_e_ok(a: Vec3, b: Vec3) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn linear_to_oklab(linear: Vec3) -> Vec3 {
    xyz_to_oklab(mul(&LIN_SRGB_TO_XYZ, linear))
}

/// CSS Color 4 gamut mapping of an OKLCH colour to gamma-encoded sRGB.
pub fn map_oklch_to_srgb(oklch: Vec3) -> Vec3 {
    let [l, chroma, hue] = oklch;
    if l >= 1.0 {
        return [1.0, 1.0, 1.0];
    }
    if l <= 0.0 {
        return [0.0, 0.0, 0.0];
    }
    let encode = |linear: Vec3| map3(linear, srgb_encode);
    let linear = oklch_to_linear_srgb(oklch);
    if in_srgb_gamut(linear) {
        return encode(linear);
    }
    let mut clipped = clip01(linear);
    let current_lab = |c: f64| polar_to_rect([l, c, hue]);
    let error = delta_e_ok(linear_to_oklab(clipped), current_lab(chroma));
    if error < JND {
        return encode(clipped);
    }
    let mut min = 0.0;
    let mut max = chroma;
    let mut min_in_gamut = true;
    while max - min > GAMUT_EPSILON {
        let probe = (min + max) / 2.0;
        let candidate = oklch_to_linear_srgb([l, probe, hue]);
        if min_in_gamut && in_srgb_gamut(candidate) {
            min = probe;
            continue;
        }
        clipped = clip01(candidate);
        let error = delta_e_ok(linear_to_oklab(clipped), current_lab(probe));
        if error < JND {
            if JND - error < GAMUT_EPSILON {
                return encode(clipped);
            }
            min_in_gamut = false;
            min = probe;
        } else {
            max = probe;
        }
    }
    encode(clipped)
}

// ---------------------------------------------------------------- color-mix()

/// The interpolation space of a `color-mix()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MixSpace {
    Srgb,
    SrgbLinear,
    Oklab,
    Oklch,
}

impl MixSpace {
    fn space(self) -> Space {
        match self {
            Self::Srgb => Space::Srgb,
            Self::SrgbLinear => Space::SrgbLinear,
            Self::Oklab => Space::Oklab,
            Self::Oklch => Space::Oklch,
        }
    }

    fn hue_index(self) -> Option<usize> {
        matches!(self, Self::Oklch).then_some(2)
    }
}

/// CSS `color-mix(in <space>, a pa%, b pb%)`. Percentages are in `0..=100`.
pub fn color_mix(space: MixSpace, a: Color, pa: Option<f64>, b: Color, pb: Option<f64>) -> Color {
    let (mut pa, mut pb) = match (pa, pb) {
        (None, None) => (50.0, 50.0),
        (Some(p), None) => (p, 100.0 - p),
        (None, Some(p)) => (100.0 - p, p),
        (Some(x), Some(y)) => (x, y),
    };
    let sum = pa + pb;
    let mut alpha_multiplier = 1.0;
    if sum == 0.0 {
        return Color::TRANSPARENT;
    }
    if sum != 100.0 {
        if sum < 100.0 {
            alpha_multiplier = sum / 100.0;
        }
        pa = pa / sum * 100.0;
        pb = pb / sum * 100.0;
    }
    let (wa, wb) = (pa / 100.0, pb / 100.0);
    let ca = a.to_space(space.space());
    let cb = b.to_space(space.space());
    let mut ha = ca.c;
    let mut hb = cb.c;
    let mut hue = None;
    if let Some(index) = space.hue_index() {
        // A hue is missing when it came out of a conversion with no chroma; one written in
        // OKLCH keeps its hue even at zero chroma, which is what Chrome interpolates.
        let a_missing = ca.c[1] <= POWERLESS_OKLCH_CHROMA && a.space != Space::Oklch;
        let b_missing = cb.c[1] <= POWERLESS_OKLCH_CHROMA && b.space != Space::Oklch;
        let (x, y) = match (a_missing, b_missing) {
            (true, true) => (0.0, 0.0),
            (true, false) => (cb.c[index], cb.c[index]),
            (false, true) => (ca.c[index], ca.c[index]),
            (false, false) => (ca.c[index], cb.c[index]),
        };
        let x = normalize_hue(x);
        let mut y = normalize_hue(y);
        if y - x > 180.0 {
            y -= 360.0;
        } else if y - x < -180.0 {
            y += 360.0;
        }
        hue = Some(x * wa + y * wb);
        ha[index] = 0.0;
        hb[index] = 0.0;
    }
    let alpha = ca.alpha * wa + cb.alpha * wb;
    let mut out = [0.0; 3];
    for i in 0..3 {
        if Some(i) == space.hue_index() {
            continue;
        }
        let premultiplied = ha[i] * ca.alpha * wa + hb[i] * cb.alpha * wb;
        out[i] = if alpha == 0.0 {
            0.0
        } else {
            premultiplied / alpha
        };
    }
    if let (Some(index), Some(h)) = (space.hue_index(), hue) {
        out[index] = normalize_hue(h);
    }
    Color {
        space: space.space(),
        c: out,
        alpha: alpha * alpha_multiplier,
    }
}

// ---------------------------------------------------------------- number formats

/// Fixed-point with trailing zeros removed and negative zero collapsed.
pub fn trim_number(value: f64, precision: usize) -> String {
    let rounded = if value.abs() < 10f64.powi(-(precision as i32)) / 2.0 {
        0.0
    } else {
        value
    };
    let text = format!("{rounded:.precision$}");
    if text.contains('.') {
        let trimmed = text.trim_end_matches('0').trim_end_matches('.');
        if trimmed.is_empty() || trimmed == "-" {
            "0".to_owned()
        } else {
            trimmed.to_owned()
        }
    } else {
        text
    }
}

// ---------------------------------------------------------------- parsing

fn parse_hex(hex: &str) -> Option<Color> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|v| v * 17);
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let (r, g, b, a) = match hex.len() {
        3 => (nibble(0)?, nibble(1)?, nibble(2)?, 255),
        4 => (nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?),
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Color::from_rgb8(r, g, b).with_alpha(f64::from(a) / 255.0))
}

#[derive(Clone, Copy, Debug)]
enum Arg {
    Number(f64),
    Percent(f64),
    Angle(f64),
    None,
}

fn parse_arg(token: &str) -> Option<Arg> {
    if token == "none" {
        return Some(Arg::None);
    }
    if let Some(p) = token.strip_suffix('%') {
        return parse_number(p).map(Arg::Percent);
    }
    for (unit, factor) in [
        ("deg", 1.0),
        ("grad", 0.9),
        ("rad", 180.0 / std::f64::consts::PI),
        ("turn", 360.0),
    ] {
        if let Some(n) = token.strip_suffix(unit) {
            return parse_number(n).map(|v| Arg::Angle(v * factor));
        }
    }
    parse_number(token).map(Arg::Number)
}

fn parse_number(text: &str) -> Option<f64> {
    if text.is_empty() || text.ends_with('.') && text.len() == 1 {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    value.is_finite().then_some(value)
}

struct Args {
    values: Vec<Arg>,
    alpha: Option<Arg>,
    legacy: bool,
}

fn split_args(body: &str) -> Option<Args> {
    let (main, alpha) = match body.split_once('/') {
        Some((m, a)) => (m, Some(a.trim())),
        None => (body, None),
    };
    let legacy = main.contains(',');
    let mut parts: Vec<&str> = if legacy {
        main.split(',').map(str::trim).collect()
    } else {
        main.split_whitespace().collect()
    };
    let mut alpha_arg = None;
    if let Some(a) = alpha {
        if legacy {
            return None;
        }
        alpha_arg = Some(parse_arg(a)?);
    } else if legacy && parts.len() == 4 {
        alpha_arg = Some(parse_arg(parts.pop()?)?);
    } else if !legacy && parts.len() == 4 {
        return None;
    }
    let values = parts
        .into_iter()
        .map(parse_arg)
        .collect::<Option<Vec<_>>>()?;
    Some(Args {
        values,
        alpha: alpha_arg,
        legacy,
    })
}

fn alpha_of(arg: Option<Arg>) -> Option<f64> {
    let alpha = match arg {
        None => 1.0,
        Some(Arg::Number(v)) => v,
        Some(Arg::Percent(p)) => p / 100.0,
        Some(Arg::None) => 0.0,
        Some(Arg::Angle(_)) => return None,
    };
    Some(alpha.clamp(0.0, 1.0))
}

fn component(arg: Arg, percent_scale: f64) -> Option<f64> {
    match arg {
        Arg::Number(v) => Some(v),
        Arg::Percent(p) => Some(p / 100.0 * percent_scale),
        Arg::None => Some(0.0),
        Arg::Angle(_) => None,
    }
}

fn hue_component(arg: Arg) -> Option<f64> {
    match arg {
        Arg::Number(v) | Arg::Angle(v) => Some(v),
        Arg::None => Some(0.0),
        Arg::Percent(_) => None,
    }
}

fn parse_function(name: &str, body: &str) -> Option<Color> {
    if name == "color" {
        return parse_color_function(body);
    }
    let args = split_args(body)?;
    if args.values.len() != 3 {
        return None;
    }
    let alpha = alpha_of(args.alpha)?;
    let v = &args.values;
    match name {
        "rgb" | "rgba" => {
            let percents = v.iter().filter(|a| matches!(a, Arg::Percent(_))).count();
            if args.legacy && percents != 0 && percents != 3 {
                return None;
            }
            let ch = |a: Arg| component(a, 255.0).map(|x| x / 255.0);
            Some(Color::new(
                Space::Srgb,
                [ch(v[0])?, ch(v[1])?, ch(v[2])?],
                alpha,
            ))
        }
        "hsl" | "hsla" => {
            let sl = |a: Arg| match a {
                Arg::Percent(p) => Some(p / 100.0),
                Arg::Number(n) if !args.legacy => Some(n / 100.0),
                Arg::None => Some(0.0),
                _ => None,
            };
            Some(Color::new(
                Space::Hsl,
                [hue_component(v[0])?, sl(v[1])?, sl(v[2])?],
                alpha,
            ))
        }
        "hwb" => {
            let wb = |a: Arg| match a {
                Arg::Percent(p) => Some(p / 100.0),
                Arg::Number(n) => Some(n / 100.0),
                Arg::None => Some(0.0),
                Arg::Angle(_) => None,
            };
            Some(Color::new(
                Space::Hwb,
                [hue_component(v[0])?, wb(v[1])?, wb(v[2])?],
                alpha,
            ))
        }
        "lab" => Some(Color::new(
            Space::Lab,
            [
                component(v[0], 100.0)?,
                component(v[1], 125.0)?,
                component(v[2], 125.0)?,
            ],
            alpha,
        )),
        "lch" => Some(Color::new(
            Space::Lch,
            [
                component(v[0], 100.0)?,
                component(v[1], 150.0)?,
                hue_component(v[2])?,
            ],
            alpha,
        )),
        "oklab" => Some(Color::new(
            Space::Oklab,
            [
                component(v[0], 1.0)?,
                component(v[1], 0.4)?,
                component(v[2], 0.4)?,
            ],
            alpha,
        )),
        "oklch" => Some(Color::new(
            Space::Oklch,
            [
                component(v[0], 1.0)?,
                component(v[1], 0.4)?,
                hue_component(v[2])?,
            ],
            alpha,
        )),
        _ => None,
    }
}

fn parse_color_function(body: &str) -> Option<Color> {
    let (main, alpha) = match body.split_once('/') {
        Some((m, a)) => (m, Some(parse_arg(a.trim())?)),
        None => (body, None),
    };
    let mut parts = main.split_whitespace();
    let space = match parts.next()? {
        "srgb" => Space::Srgb,
        "srgb-linear" => Space::SrgbLinear,
        "display-p3" => Space::DisplayP3,
        "a98-rgb" => Space::A98Rgb,
        "prophoto-rgb" => Space::ProphotoRgb,
        "rec2020" => Space::Rec2020,
        "xyz" | "xyz-d65" => Space::XyzD65,
        "xyz-d50" => Space::XyzD50,
        _ => return None,
    };
    let rest = parts.map(parse_arg).collect::<Option<Vec<_>>>()?;
    if rest.len() != 3 {
        return None;
    }
    let ch = |a: Arg| component(a, 1.0);
    Some(Color::new(
        space,
        [ch(rest[0])?, ch(rest[1])?, ch(rest[2])?],
        alpha_of(alpha)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_named_parse() {
        assert_eq!(
            Color::parse("#fff").map(|c| c.to_hex()),
            Some("#ffffff".into())
        );
        assert_eq!(
            Color::parse("#0a0A0a80").map(|c| c.to_rgba8()),
            Some([10, 10, 10, 128])
        );
        assert_eq!(
            Color::parse("rebeccapurple").map(|c| c.to_hex()),
            Some("#663399".into())
        );
        assert!(Color::parse("#12").is_none());
        assert!(Color::parse("var(--x)").is_none());
        assert!(Color::parse("rgb(10%, 20, 30)").is_none());
    }

    #[test]
    fn trim_number_matches_the_typescript_formatter() {
        assert_eq!(trim_number(0.5, 6), "0.5");
        assert_eq!(trim_number(1.0, 6), "1");
        assert_eq!(trim_number(-0.0000001, 6), "0");
        assert_eq!(trim_number(325.6530004, 3), "325.653");
        assert_eq!(trim_number(100.0, 3), "100");
    }

    #[test]
    fn mixing_in_srgb_is_premultiplied() {
        let mixed = color_mix(
            MixSpace::Srgb,
            Color::srgb(1.0, 0.0, 0.0),
            Some(50.0),
            Color::TRANSPARENT,
            None,
        );
        assert_eq!(mixed.c, [1.0, 0.0, 0.0]);
        assert!((mixed.alpha - 0.5).abs() < 1e-12);
    }
}
