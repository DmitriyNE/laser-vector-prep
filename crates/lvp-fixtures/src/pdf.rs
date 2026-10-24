//! Synthetic vector PDF fixture built with `lopdf` (independent of the core's reader and
//! writer): device colours, a form XObject with a matrix, Béziers, even-odd fill, clipping,
//! an optional-content layer, plus text and an image that must be diagnosed.

use lopdf::{Document, Object, Stream, dictionary};
use serde_json::{Value, json};

pub struct PdfFixture {
    pub name: &'static str,
    pub description: &'static str,
    pub bytes: Vec<u8>,
    pub expect: Value,
    pub notes: Vec<&'static str>,
}

impl PdfFixture {
    pub fn file_name(&self) -> String {
        format!("pdf/{}.pdf", self.name)
    }
}

const PT: f64 = 25.4 / 72.0;

fn features() -> PdfFixture {
    let mut doc = Document::with_version("1.5");
    // Deterministic output: lopdf writes objects in id order, no dates or ids are added.
    let pages_id = doc.new_object_id();
    let form = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]),
            "Matrix" => Object::Array(vec![Object::Real(2.0), 0.into(), 0.into(), Object::Real(2.0), 0.into(), 0.into()]),
        },
        b"0 0 10 10 re f".to_vec(),
    ));
    let ocg = doc
        .add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Outline") });
    let image = doc.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 },
        vec![128u8],
    ));
    let resources = dictionary! {
        "XObject" => dictionary! { "Pad" => form, "Im" => image },
        "Properties" => dictionary! { "oc1" => ocg },
    };
    let k = 0.552_284_75 * 36.0;
    let content = format!(
        "%% blue 1 pt stroked closed path (a 72 pt square) on the Outline layer\n\
         /OC /oc1 BDC 0 0 1 RG 1 w 36 36 m 108 36 l 108 108 l 36 108 l h S EMC\n\
         %% red Bézier circle r = 36 pt at (216, 72), filled\n\
         1 0 0 rg 252 72 m 252 {a} {a2} 108 216 108 c {b} 108 180 {a} 180 72 c 180 {c} {b} 36 216 36 c {a2} 36 252 {c} 252 72 c f\n\
         %% black even-odd ring 324..396 x 36..108 with a 20 pt hole\n\
         0 g 324 36 72 72 re 350 62 20 20 re f*\n\
         %% two scaled form pads (20 x 20 pt) in green\n\
         0 0.6 0 rg q 1 0 0 1 36 144 cm /Pad Do Q q 1 0 0 1 72 144 cm /Pad Do Q\n\
         %% clipped grey fill: rect 144..216 x 144..216 clipped to x >= 180\n\
         q 180 144 36 72 re W n 0.5 g 144 144 72 72 re f Q\n\
         %% text and an image: diagnosed, not geometry\n\
         BT /F1 12 Tf 324 144 Td (label) Tj ET\n\
         q 20 0 0 20 400 150 cm /Im Do Q\n",
        a = 72.0 + k,
        a2 = 216.0 + k,
        b = 216.0 - k,
        c = 72.0 - k
    );
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "MediaBox" => Object::Array(vec![0.into(), 0.into(), 432.into(), 216.into()]),
        "Resources" => resources,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
        ),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("in-memory PDF");

    let circle = std::f64::consts::PI * (36.0 * PT) * (36.0 * PT);
    PdfFixture {
        name: "features",
        description: "One page, 432 x 216 pt: an optional-content layer with a stroked square, a red Bézier circle, a black even-odd ring, two green pads from a scaled form XObject, a grey fill cut by a clipping path, plus text and an image that must be diagnosed.",
        bytes,
        expect: json!({
            "layers": ["#000000", "#009900", "#808080", "#ff0000", "Outline"],
            "outline_paths": 1,
            "outline_width_mm": 1.0 * PT,
            "outline_side_mm": 72.0 * PT,
            "red_area_mm2": circle,
            "black_area_mm2": (72.0 * 72.0 - 20.0 * 20.0) * PT * PT,
            "black_holes": 1,
            "green_area_mm2": 2.0 * 20.0 * 20.0 * PT * PT,
            "grey_area_mm2": 36.0 * 72.0 * PT * PT,
            "grey_bounds_mm": [180.0 * PT, 144.0 * PT, 216.0 * PT, 216.0 * PT],
            "diagnostics": ["pdf.text", "pdf.image"],
        }),
        notes: vec![
            "layers are optional-content group names where present, otherwise the paint colour as #rrggbb",
            "the Bézier circle area is exact to the kappa approximation (about 0.03 %)",
            "0.6 green rounds to 0x99; CMYK and gray paints are converted to RGB names the same way",
        ],
    }
}

pub fn all() -> Vec<PdfFixture> {
    vec![features()]
}
