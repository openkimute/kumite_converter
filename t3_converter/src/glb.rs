//! The minimal glTF 2.0 binary writer of export_gltf (`GLB`): the same JSON, views, padding and bytes.

use crate::pyjson;
use serde_json::{Map, Value, json};

pub struct Glb {
    pub bin: Vec<u8>,
    /// The document; its `bufferViews` and `accessors` keep their place and are filled from the lists below on
    /// writing.
    pub j: Map<String, Value>,
    pub views: Vec<Value>,
    pub accessors: Vec<Value>,
}

impl Default for Glb {
    fn default() -> Self {
        Self::new("t3_model_extractor")
    }
}

impl Glb {
    /// `generator`: the asset's generator string (it is part of the online
    /// fingerprint).
    pub fn new(generator: &str) -> Glb {
        let mut j = Map::new();
        j.insert("asset".into(), json!({"version": "2.0", "generator": generator}));
        j.insert("bufferViews".into(), json!([]));
        j.insert("accessors".into(), json!([]));
        Glb { bin: Vec::new(), j, views: Vec::new(), accessors: Vec::new() }
    }

    pub fn view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = Map::new();
        v.insert("buffer".into(), json!(0));
        v.insert("byteOffset".into(), json!(self.bin.len()));
        v.insert("byteLength".into(), json!(data.len()));
        if let Some(t) = target.filter(|t| *t != 0) {
            v.insert("target".into(), json!(t));
        }
        self.bin.extend_from_slice(data);
        self.views.push(Value::Object(v));
        self.views.len() - 1
    }

    /// An accessor over `count` elements of `bytes` (little-endian, packed). `minmax`: per-component min and max,
    /// given by the caller as floats (Python: `float(v)` of the array's min/max).
    #[allow(clippy::too_many_arguments)]
    pub fn acc(
        &mut self,
        bytes: &[u8],
        count: usize,
        ctype: u32,
        typ: &str,
        target: Option<u32>,
        minmax: Option<(Vec<f64>, Vec<f64>)>,
        normalized: bool,
    ) -> usize {
        let view = self.view(bytes, target);
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(ctype));
        a.insert("count".into(), json!(count));
        a.insert("type".into(), json!(typ));
        if normalized {
            a.insert("normalized".into(), json!(true));
        }
        if let Some((min, max)) = minmax {
            a.insert("min".into(), Value::Array(min.into_iter().map(f64_value).collect()));
            a.insert("max".into(), Value::Array(max.into_iter().map(f64_value).collect()));
        }
        self.accessors.push(Value::Object(a));
        self.accessors.len() - 1
    }

    pub fn to_bytes(&mut self) -> Vec<u8> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.j.insert("bufferViews".into(), Value::Array(self.views.clone()));
        self.j.insert("accessors".into(), Value::Array(self.accessors.clone()));
        self.j.insert("buffers".into(), json!([{"byteLength": self.bin.len()}]));
        let mut js = pyjson::compact(&Value::Object(self.j.clone())).into_bytes();
        while !js.len().is_multiple_of(4) {
            js.push(b' ');
        }
        let mut out = Vec::with_capacity(28 + js.len() + self.bin.len());
        out.extend(0x46546C67u32.to_le_bytes());
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + js.len() + 8 + self.bin.len()) as u32).to_le_bytes());
        out.extend((js.len() as u32).to_le_bytes());
        out.extend(0x4E4F534Au32.to_le_bytes());
        out.extend(js);
        out.extend((self.bin.len() as u32).to_le_bytes());
        out.extend(0x004E4942u32.to_le_bytes());
        out.extend(&self.bin);
        out
    }
}

/// A float as a JSON number (a non-finite one as null: Python would write NaN, which glTF readers refuse).
pub fn f64_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// Little-endian bytes of f32s / u16s / u32s (numpy's `tobytes()`).
pub fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
pub fn u16_bytes(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
pub fn u32_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
