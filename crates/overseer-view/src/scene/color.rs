//! CSS colours of room visuals: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()` /
//! `rgba()` (numbers or percentages) and the CSS named colours; `transparent` is no colour.

/// An sRGB colour with alpha.
pub type Rgba = [u8; 4];

/// Parses a CSS colour; `None` when it is not one (or is `transparent`).
pub fn parse(text: &str) -> Option<Rgba> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = text.to_ascii_lowercase();
    if let Some(arguments) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return parse_rgb(arguments);
    }
    if lower == "transparent" {
        return None;
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, rgb)| {
            let [_, r, g, b] = rgb.to_be_bytes();
            [r, g, b, 255]
        })
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let digit = |c: char| c.to_digit(16).and_then(|value| u8::try_from(value).ok());
    let digits: Vec<u8> = hex.chars().map(digit).collect::<Option<_>>()?;
    match digits.as_slice() {
        [r, g, b] => Some([r * 17, g * 17, b * 17, 255]),
        [r, g, b, a] => Some([r * 17, g * 17, b * 17, a * 17]),
        [r1, r2, g1, g2, b1, b2] => Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, 255]),
        [r1, r2, g1, g2, b1, b2, a1, a2] => {
            Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, a1 * 16 + a2])
        }
        _ => None,
    }
}

fn parse_rgb(arguments: &str) -> Option<Rgba> {
    let parts: Vec<&str> = arguments
        .split([',', ' ', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    let channel = |part: &str, scale: f32| -> Option<u8> {
        let value = match part.strip_suffix('%') {
            Some(percent) => percent.parse::<f32>().ok()? / 100.0 * scale,
            None => part.parse::<f32>().ok()?,
        };
        // Clamped to 0..=255 before the cast.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 0..=255"
        )]
        let byte = value.clamp(0.0, 255.0).round() as u8;
        Some(byte)
    };
    let (r, g, b) = (
        channel(parts.first()?, 255.0)?,
        channel(parts.get(1)?, 255.0)?,
        channel(parts.get(2)?, 255.0)?,
    );
    let alpha = match parts.get(3) {
        Some(alpha) => match alpha.strip_suffix('%') {
            Some(_) => channel(alpha, 255.0)?,
            None => unit(alpha.parse::<f32>().ok()?),
        },
        None => 255,
    };
    Some([r, g, b, alpha])
}

/// A fraction (`0..=1`, clamped) as a byte.
pub fn unit(value: f32) -> u8 {
    // Clamped to 0..=255 before the cast.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255"
    )]
    let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    byte
}

/// The CSS named colours (sRGB).
const NAMED: [(&str, u32); 148] = [
    ("aliceblue", 0xf0_f8_ff),
    ("antiquewhite", 0xfa_eb_d7),
    ("aqua", 0x00_ff_ff),
    ("aquamarine", 0x7f_ff_d4),
    ("azure", 0xf0_ff_ff),
    ("beige", 0xf5_f5_dc),
    ("bisque", 0xff_e4_c4),
    ("black", 0x00_00_00),
    ("blanchedalmond", 0xff_eb_cd),
    ("blue", 0x00_00_ff),
    ("blueviolet", 0x8a_2b_e2),
    ("brown", 0xa5_2a_2a),
    ("burlywood", 0xde_b8_87),
    ("cadetblue", 0x5f_9e_a0),
    ("chartreuse", 0x7f_ff_00),
    ("chocolate", 0xd2_69_1e),
    ("coral", 0xff_7f_50),
    ("cornflowerblue", 0x64_95_ed),
    ("cornsilk", 0xff_f8_dc),
    ("crimson", 0xdc_14_3c),
    ("cyan", 0x00_ff_ff),
    ("darkblue", 0x00_00_8b),
    ("darkcyan", 0x00_8b_8b),
    ("darkgoldenrod", 0xb8_86_0b),
    ("darkgray", 0xa9_a9_a9),
    ("darkgreen", 0x00_64_00),
    ("darkgrey", 0xa9_a9_a9),
    ("darkkhaki", 0xbd_b7_6b),
    ("darkmagenta", 0x8b_00_8b),
    ("darkolivegreen", 0x55_6b_2f),
    ("darkorange", 0xff_8c_00),
    ("darkorchid", 0x99_32_cc),
    ("darkred", 0x8b_00_00),
    ("darksalmon", 0xe9_96_7a),
    ("darkseagreen", 0x8f_bc_8f),
    ("darkslateblue", 0x48_3d_8b),
    ("darkslategray", 0x2f_4f_4f),
    ("darkslategrey", 0x2f_4f_4f),
    ("darkturquoise", 0x00_ce_d1),
    ("darkviolet", 0x94_00_d3),
    ("deeppink", 0xff_14_93),
    ("deepskyblue", 0x00_bf_ff),
    ("dimgray", 0x69_69_69),
    ("dimgrey", 0x69_69_69),
    ("dodgerblue", 0x1e_90_ff),
    ("firebrick", 0xb2_22_22),
    ("floralwhite", 0xff_fa_f0),
    ("forestgreen", 0x22_8b_22),
    ("fuchsia", 0xff_00_ff),
    ("gainsboro", 0xdc_dc_dc),
    ("ghostwhite", 0xf8_f8_ff),
    ("gold", 0xff_d7_00),
    ("goldenrod", 0xda_a5_20),
    ("gray", 0x80_80_80),
    ("green", 0x00_80_00),
    ("greenyellow", 0xad_ff_2f),
    ("grey", 0x80_80_80),
    ("honeydew", 0xf0_ff_f0),
    ("hotpink", 0xff_69_b4),
    ("indianred", 0xcd_5c_5c),
    ("indigo", 0x4b_00_82),
    ("ivory", 0xff_ff_f0),
    ("khaki", 0xf0_e6_8c),
    ("lavender", 0xe6_e6_fa),
    ("lavenderblush", 0xff_f0_f5),
    ("lawngreen", 0x7c_fc_00),
    ("lemonchiffon", 0xff_fa_cd),
    ("lightblue", 0xad_d8_e6),
    ("lightcoral", 0xf0_80_80),
    ("lightcyan", 0xe0_ff_ff),
    ("lightgoldenrodyellow", 0xfa_fa_d2),
    ("lightgray", 0xd3_d3_d3),
    ("lightgreen", 0x90_ee_90),
    ("lightgrey", 0xd3_d3_d3),
    ("lightpink", 0xff_b6_c1),
    ("lightsalmon", 0xff_a0_7a),
    ("lightseagreen", 0x20_b2_aa),
    ("lightskyblue", 0x87_ce_fa),
    ("lightslategray", 0x77_88_99),
    ("lightslategrey", 0x77_88_99),
    ("lightsteelblue", 0xb0_c4_de),
    ("lightyellow", 0xff_ff_e0),
    ("lime", 0x00_ff_00),
    ("limegreen", 0x0032_cd32),
    ("linen", 0xfa_f0_e6),
    ("magenta", 0xff_00_ff),
    ("maroon", 0x80_00_00),
    ("mediumaquamarine", 0x66_cd_aa),
    ("mediumblue", 0x00_00_cd),
    ("mediumorchid", 0xba_55_d3),
    ("mediumpurple", 0x93_70_db),
    ("mediumseagreen", 0x3c_b3_71),
    ("mediumslateblue", 0x7b_68_ee),
    ("mediumspringgreen", 0x00_fa_9a),
    ("mediumturquoise", 0x48_d1_cc),
    ("mediumvioletred", 0xc7_15_85),
    ("midnightblue", 0x19_19_70),
    ("mintcream", 0xf5_ff_fa),
    ("mistyrose", 0xff_e4_e1),
    ("moccasin", 0xff_e4_b5),
    ("navajowhite", 0xff_de_ad),
    ("navy", 0x00_00_80),
    ("oldlace", 0xfd_f5_e6),
    ("olive", 0x80_80_00),
    ("olivedrab", 0x6b_8e_23),
    ("orange", 0xff_a5_00),
    ("orangered", 0xff_45_00),
    ("orchid", 0xda_70_d6),
    ("palegoldenrod", 0xee_e8_aa),
    ("palegreen", 0x98_fb_98),
    ("paleturquoise", 0xaf_ee_ee),
    ("palevioletred", 0xdb_70_93),
    ("papayawhip", 0xff_ef_d5),
    ("peachpuff", 0xff_da_b9),
    ("peru", 0xcd_85_3f),
    ("pink", 0xff_c0_cb),
    ("plum", 0xdd_a0_dd),
    ("powderblue", 0xb0_e0_e6),
    ("purple", 0x80_00_80),
    ("rebeccapurple", 0x66_33_99),
    ("red", 0xff_00_00),
    ("rosybrown", 0xbc_8f_8f),
    ("royalblue", 0x41_69_e1),
    ("saddlebrown", 0x8b_45_13),
    ("salmon", 0xfa_80_72),
    ("sandybrown", 0xf4_a4_60),
    ("seagreen", 0x2e_8b_57),
    ("seashell", 0xff_f5_ee),
    ("sienna", 0xa0_52_2d),
    ("silver", 0xc0_c0_c0),
    ("skyblue", 0x87_ce_eb),
    ("slateblue", 0x6a_5a_cd),
    ("slategray", 0x70_80_90),
    ("slategrey", 0x70_80_90),
    ("snow", 0xff_fa_fa),
    ("springgreen", 0x00_ff_7f),
    ("steelblue", 0x46_82_b4),
    ("tan", 0xd2_b4_8c),
    ("teal", 0x00_80_80),
    ("thistle", 0xd8_bf_d8),
    ("tomato", 0xff_63_47),
    ("turquoise", 0x40_e0_d0),
    ("violet", 0xee_82_ee),
    ("wheat", 0xf5_de_b3),
    ("white", 0xff_ff_ff),
    ("whitesmoke", 0xf5_f5_f5),
    ("yellow", 0xff_ff_00),
    ("yellowgreen", 0x009a_cd32),
];
