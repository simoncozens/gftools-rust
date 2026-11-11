/// Known font weights in the Google Fonts API and their corresponding numeric values.
pub const KNOWN_WEIGHTS: [(&str, u16); 11] = [
    ("Thin", 100),
    ("Hairline", 100),
    ("ExtraLight", 200),
    ("Light", 300),
    ("Regular", 400),
    ("", 400), // Family-Italic resolves to this
    ("Medium", 500),
    ("SemiBold", 600),
    ("Bold", 700),
    ("ExtraBold", 800),
    ("Black", 900),
];
