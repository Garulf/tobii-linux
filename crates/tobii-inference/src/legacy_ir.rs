//! The researched IR4/IR6 graphs of Tobii's networks, translated into OpenVINO IR v11.
//!
//! OpenVINO no longer reads IR versions below 10, and its C API cannot build graphs. The
//! translation rebuilds exactly the opset1 operations the Python loader built with
//! OpenVINO's Python opset API, layer by layer: the same operations, attributes, constant
//! bytes and topology, so the plugin compiles the same graph. Only the observed layer types
//! are known; anything else fails, as do hash mismatches, unconnected or doubly connected
//! inputs, blobs outside the weights file and every declared shape the operations disagree
//! with.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{Result, value};

/// A translated network: the IR v11 document and its weights, ready for OpenVINO's
/// `read_model`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrModel {
    pub name: Option<String>,
    pub xml: String,
    pub weights: Vec<u8>,
    /// Shapes of the float32 parameters, in the order the Python loader created them.
    pub inputs: Vec<Vec<i64>>,
}

/// Translates the model directory (`model.xml`, `model.bin` and the `manifest.json`
/// holding their SHA-256), checking both hashes first.
pub fn load(directory: &Path) -> Result<IrModel> {
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)
        .map_err(|error| crate::Error::Value(format!("Invalid model manifest: {error}")))?;
    let mut files = Vec::new();
    for filename in ["model.xml", "model.bin"] {
        let data = std::fs::read(directory.join(filename))?;
        let expected = manifest["files"][filename]["sha256"].as_str();
        if expected != Some(sha256_hex(&data).as_str()) {
            return value(format!("Model hash mismatch: {filename}"));
        }
        files.push(data);
    }
    let text = String::from_utf8(files.remove(0)).or_else(|_| value("Model XML is not UTF-8"))?;
    translate(&text, &files[0])
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Translates one legacy IR document and its weights.
pub fn translate(xml: &str, weights: &[u8]) -> Result<IrModel> {
    let document = roxmltree::Document::parse(xml).or_else(|error| value(format!("Invalid model XML: {error}")))?;
    let root = document.root_element();
    if !matches!(root.attribute("version"), Some("4" | "6")) {
        return value("Expected researched IR4/IR6 graph");
    }
    let mut builder = Builder { weights, graph: Graph::default() };
    builder.run(root)?;
    let name = root.attribute("name").map(str::to_owned);
    Ok(builder.graph.serialize(name.as_deref().unwrap_or("")).with_name(name))
}

impl IrModel {
    fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }
}

fn children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> {
    node.children().filter(move |child| child.has_tag_name(tag))
}

/// ElementTree's `findall('./<outer>/<inner>')`.
fn find_all<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    outer: &'static str,
    inner: &'static str,
) -> Vec<roxmltree::Node<'a, 'input>> {
    children(node, outer).flat_map(|child| children(child, inner)).collect()
}

/// Python's `int(text)`: surrounding whitespace allowed.
fn int(text: Option<&str>) -> Result<i64> {
    match text {
        Some(text) => text.trim().parse().or_else(|_| value(format!("invalid literal for int(): {text:?}"))),
        None => value("Missing integer attribute"),
    }
}

fn shape(port: roxmltree::Node<'_, '_>) -> Result<Vec<i64>> {
    children(port, "dim").map(|dim| int(Some(dim.text().unwrap_or("")))).collect()
}

fn port_id(port: roxmltree::Node<'_, '_>) -> Result<i64> {
    int(port.attribute("id"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Element {
    F32,
    I32,
    I64,
}

impl Element {
    fn size(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I64 => 8,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::I32 => "i32",
            Self::I64 => "i64",
        }
    }

    fn precision(self) -> &'static str {
        match self {
            Self::F32 => "FP32",
            Self::I32 => "I32",
            Self::I64 => "I64",
        }
    }
}

#[derive(Debug, Clone)]
struct Window {
    strides: Vec<i64>,
    pads_begin: Vec<i64>,
    pads_end: Vec<i64>,
}

#[derive(Debug, Clone)]
enum Op {
    Parameter,
    Constant { data: Vec<u8> },
    Convolution { window: Window, dilations: Vec<i64>, grouped: bool },
    Pool { window: Window, kernel: Vec<i64>, ceil: bool, average: Option<bool> },
    Add,
    Relu,
    PRelu,
    Sigmoid,
    Reshape,
    MatMul,
    Softmax { axis: i64 },
    Transpose,
    Concat { axis: i64 },
    Result,
}

#[derive(Debug)]
struct Node {
    name: String,
    op: Op,
    inputs: Vec<usize>,
    shape: Vec<i64>,
    element: Element,
}

/// The rebuilt graph: nodes in creation order, each with one output.
#[derive(Debug, Default)]
struct Graph {
    nodes: Vec<Node>,
}

impl Graph {
    fn add(&mut self, name: String, op: Op, inputs: Vec<usize>, shape: Vec<i64>, element: Element) -> usize {
        self.nodes.push(Node { name, op, inputs, shape, element });
        self.nodes.len() - 1
    }

    fn shape(&self, node: usize) -> &[i64] {
        &self.nodes[node].shape
    }

    fn constant_values(&self, node: usize) -> Option<Vec<i64>> {
        let Node { op: Op::Constant { data }, element, .. } = &self.nodes[node] else { return None };
        let values = match element {
            Element::I32 => data.as_chunks::<4>().0.iter().map(|c| i64::from(i32::from_le_bytes(*c))).collect(),
            Element::I64 => data.as_chunks::<8>().0.iter().map(|c| i64::from_le_bytes(*c)).collect(),
            Element::F32 => return None,
        };
        Some(values)
    }

    fn constant(&mut self, name: String, element: Element, shape: Vec<i64>, data: Vec<u8>) -> usize {
        self.add(name, Op::Constant { data }, Vec::new(), shape, element)
    }

    fn int64s(&mut self, name: String, values: &[i64]) -> usize {
        let data = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.constant(name, Element::I64, vec![values.len() as i64], data)
    }

    fn add_op(&mut self, name: String, left: usize, right: usize) -> Result<usize> {
        let shape = broadcast(self.shape(left), self.shape(right))?;
        Ok(self.add(name, Op::Add, vec![left, right], shape, Element::F32))
    }

    fn serialize(&self, name: &str) -> IrModel {
        let mut xml = format!("<?xml version=\"1.0\"?>\n<net name=\"{}\" version=\"11\">\n\t<layers>\n", escape(name));
        let mut weights = Vec::new();
        let mut edges = String::new();
        for (id, node) in self.nodes.iter().enumerate() {
            let (kind, data) = node.attributes(&mut weights);
            let _ = writeln!(
                xml,
                "\t\t<layer id=\"{id}\" name=\"{}\" type=\"{kind}\" version=\"opset1\">",
                escape(&node.name)
            );
            if let Some(data) = data {
                let _ = writeln!(xml, "\t\t\t<data {data} />");
            }
            if !node.inputs.is_empty() {
                xml.push_str("\t\t\t<input>\n");
                for (port, &source) in node.inputs.iter().enumerate() {
                    port_xml(&mut xml, port, &self.nodes[source], "");
                    let from = self.nodes[source].inputs.len();
                    let _ = writeln!(
                        edges,
                        "\t\t<edge from-layer=\"{source}\" from-port=\"{from}\" to-layer=\"{id}\" to-port=\"{port}\" />"
                    );
                }
                xml.push_str("\t\t\t</input>\n");
            }
            if !matches!(node.op, Op::Result) {
                let names = match node.op {
                    Op::Parameter => format!(" names=\"{}\"", escape(&node.name)),
                    _ => String::new(),
                };
                xml.push_str("\t\t\t<output>\n");
                port_xml(&mut xml, node.inputs.len(), node, &names);
                xml.push_str("\t\t\t</output>\n");
            }
            xml.push_str("\t\t</layer>\n");
        }
        let _ = write!(xml, "\t</layers>\n\t<edges>\n{edges}\t</edges>\n\t<rt_info />\n</net>\n");
        let inputs = self.nodes.iter().filter(|n| matches!(n.op, Op::Parameter)).map(|n| n.shape.clone()).collect();
        IrModel { name: None, xml, weights, inputs }
    }
}

fn port_xml(xml: &mut String, id: usize, node: &Node, names: &str) {
    let _ = writeln!(xml, "\t\t\t\t<port id=\"{id}\" precision=\"{}\"{names}>", node.element.precision());
    for dim in &node.shape {
        let _ = writeln!(xml, "\t\t\t\t\t<dim>{dim}</dim>");
    }
    xml.push_str("\t\t\t\t</port>\n");
}

fn list(values: &[i64]) -> String {
    values.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl Node {
    /// The IR type and `<data>` attributes; constants append their bytes to the weights.
    fn attributes(&self, weights: &mut Vec<u8>) -> (&'static str, Option<String>) {
        let window = |w: &Window| {
            format!(
                "strides=\"{}\" pads_begin=\"{}\" pads_end=\"{}\"",
                list(&w.strides),
                list(&w.pads_begin),
                list(&w.pads_end)
            )
        };
        match &self.op {
            Op::Parameter => {
                ("Parameter", Some(format!("shape=\"{}\" element_type=\"{}\"", list(&self.shape), self.element.name())))
            }
            Op::Constant { data } => {
                let offset = weights.len();
                weights.extend_from_slice(data);
                let attributes = format!(
                    "element_type=\"{}\" shape=\"{}\" offset=\"{offset}\" size=\"{}\"",
                    self.element.name(),
                    list(&self.shape),
                    data.len()
                );
                ("Const", Some(attributes))
            }
            Op::Convolution { window: w, dilations, grouped } => {
                let kind = if *grouped { "GroupConvolution" } else { "Convolution" };
                (kind, Some(format!("{} dilations=\"{}\" auto_pad=\"explicit\"", window(w), list(dilations))))
            }
            Op::Pool { window: w, kernel, ceil, average } => {
                let rounding = if *ceil { "ceil" } else { "floor" };
                let mut attributes = format!(
                    "{} kernel=\"{}\" rounding_type=\"{rounding}\" auto_pad=\"explicit\"",
                    window(w),
                    list(kernel)
                );
                let kind = match average {
                    Some(exclude) => {
                        let _ = write!(attributes, " exclude-pad=\"{exclude}\"");
                        "AvgPool"
                    }
                    None => "MaxPool",
                };
                (kind, Some(attributes))
            }
            Op::Add => ("Add", Some("auto_broadcast=\"numpy\"".into())),
            Op::Relu => ("ReLU", None),
            Op::PRelu => ("PReLU", None),
            Op::Sigmoid => ("Sigmoid", None),
            Op::Reshape => ("Reshape", Some("special_zero=\"true\"".into())),
            Op::MatMul => ("MatMul", Some("transpose_a=\"false\" transpose_b=\"true\"".into())),
            Op::Softmax { axis } => ("SoftMax", Some(format!("axis=\"{axis}\""))),
            Op::Transpose => ("Transpose", None),
            Op::Concat { axis } => ("Concat", Some(format!("axis=\"{axis}\""))),
            Op::Result => ("Result", None),
        }
    }
}

fn broadcast(left: &[i64], right: &[i64]) -> Result<Vec<i64>> {
    let rank = left.len().max(right.len());
    let dim = |shape: &[i64], i: usize| if i + shape.len() < rank { 1 } else { shape[i + shape.len() - rank] };
    (0..rank)
        .map(|i| match (dim(left, i), dim(right, i)) {
            (a, b) if a == b || b == 1 => Ok(a),
            (1, b) => Ok(b),
            _ => value(format!("Incompatible broadcast shapes {left:?} and {right:?}")),
        })
        .collect()
}

/// Output extent of a convolution or pooling window along one axis.
fn window_extent(input: i64, pads: i64, window: i64, stride: i64, ceil: bool) -> Result<i64> {
    if stride <= 0 || input + pads < window {
        return value("Invalid window geometry");
    }
    let span = input + pads - window;
    Ok(if ceil { (span + stride - 1) / stride } else { span / stride } + 1)
}

fn spatial(input: &[i64], w: &Window, windows: &[i64], ceil: bool) -> Result<Vec<i64>> {
    let rank = input.len().saturating_sub(2);
    if [w.strides.len(), w.pads_begin.len(), w.pads_end.len(), windows.len()].iter().any(|&n| n != rank) {
        return value("Window attributes do not match the input rank");
    }
    (0..rank)
        .map(|i| window_extent(input[i + 2], w.pads_begin[i] + w.pads_end[i], windows[i], w.strides[i], ceil))
        .collect()
}

fn reshape(input: &[i64], pattern: &[i64]) -> Result<Vec<i64>> {
    let mut shape: Vec<i64> = pattern
        .iter()
        .enumerate()
        .map(|(i, &d)| if d == 0 { input.get(i).copied().ok_or(()) } else { Ok(d) })
        .collect::<Result<_, ()>>()
        .or_else(|()| value("Reshape pattern copies a missing dimension"))?;
    let total: i64 = input.iter().product();
    let known: i64 = shape.iter().filter(|&&d| d != -1).product();
    match shape.iter().filter(|&&d| d == -1).count() {
        0 if known == total => Ok(shape),
        1 if known > 0 && total % known == 0 => {
            for d in &mut shape {
                if *d == -1 {
                    *d = total / known;
                }
            }
            Ok(shape)
        }
        _ => value(format!("Cannot reshape {input:?} to {pattern:?}")),
    }
}

fn axis(axis: i64, rank: usize) -> Result<usize> {
    let rank = rank as i64;
    let normalized = if axis < 0 { axis + rank } else { axis };
    if (0..rank).contains(&normalized) { Ok(normalized as usize) } else { value(format!("Axis {axis} out of range")) }
}

/// Python's repr of a shape list, as the loader's messages print it.
fn repr(shape: &[i64]) -> String {
    format!("[{}]", shape.iter().map(i64::to_string).collect::<Vec<_>>().join(", "))
}

struct Builder<'w> {
    weights: &'w [u8],
    graph: Graph,
}

type Port = (String, i64);

struct Layer<'a, 'input> {
    node: roxmltree::Node<'a, 'input>,
    id: String,
    name: String,
    attributes: Vec<(String, String)>,
}

impl Layer<'_, '_> {
    fn get(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(key, _)| key == name).map(|(_, v)| v.as_str())
    }

    fn ints(&self, name: &str, default: Option<&str>) -> Result<Vec<i64>> {
        let Some(text) = self.get(name).or(default) else { return value(format!("Missing attribute {name}")) };
        text.split(',').map(|item| int(Some(item))).collect()
    }

    fn required(&self, name: &str) -> Result<&str> {
        self.get(name).map_or_else(|| value(format!("Missing attribute {name}")), Ok)
    }

    fn window(&self) -> Result<Window> {
        Ok(Window {
            strides: self.ints("strides", None)?,
            pads_begin: self.ints("pads_begin", None)?,
            pads_end: self.ints("pads_end", None)?,
        })
    }

    /// Python's repr of the attribute dict, for the unsupported-operation message.
    fn attributes_repr(&self) -> String {
        let items: Vec<String> = self.attributes.iter().map(|(k, v)| format!("'{k}': '{v}'")).collect();
        format!("{{{}}}", items.join(", "))
    }

    fn part(&self, suffix: &str) -> String {
        format!("{}/{suffix}", self.name)
    }
}

fn blob(weights: &[u8], layer: &Layer<'_, '_>, tag: &str, dims: &[i64], element: Element) -> Result<Vec<u8>> {
    let Some(item) = children(layer.node, "blobs").flat_map(|b| b.children().filter(|c| c.has_tag_name(tag))).next()
    else {
        return value(format!("Missing blob {}/{tag}", layer.name));
    };
    let offset = int(item.attribute("offset"))?;
    let size = int(item.attribute("size"))?;
    let count: i64 = dims.iter().product();
    if offset < 0 || size != count * element.size() as i64 || offset + size > weights.len() as i64 {
        return value("Invalid blob extent");
    }
    Ok(weights[offset as usize..(offset + size) as usize].to_vec())
}

impl Builder<'_> {
    fn has_blob(layer: &Layer<'_, '_>, tag: &str) -> bool {
        children(layer.node, "blobs").any(|b| b.children().any(|c| c.has_tag_name(tag)))
    }

    fn run(&mut self, root: roxmltree::Node<'_, '_>) -> Result<()> {
        let layers = find_all(root, "layers", "layer");
        let mut pending: Vec<(String, roxmltree::Node<'_, '_>)> = Vec::new();
        for layer in &layers {
            let id = layer.attribute("id").unwrap_or_default().to_owned();
            if pending.iter().any(|(existing, _)| *existing == id) {
                return value("Duplicate layer IDs");
            }
            pending.push((id, *layer));
        }
        let mut edges: HashMap<Port, Port> = HashMap::new();
        let mut consumed: HashSet<Port> = HashSet::new();
        for edge in find_all(root, "edges", "edge") {
            let destination =
                (edge.attribute("to-layer").unwrap_or_default().to_owned(), int(edge.attribute("to-port"))?);
            let source =
                (edge.attribute("from-layer").unwrap_or_default().to_owned(), int(edge.attribute("from-port"))?);
            if edges.contains_key(&destination) {
                return value("Multiply connected input");
            }
            edges.insert(destination, source.clone());
            consumed.insert(source);
        }
        let mut values: Vec<(Port, usize)> = Vec::new();
        while !pending.is_empty() {
            let mut progressed = false;
            let mut index = 0;
            while index < pending.len() {
                let (id, node) = pending[index].clone();
                let mut inputs = children(node, "input").flat_map(|i| children(i, "port")).collect::<Vec<_>>();
                let ids = inputs.iter().map(|&p| port_id(p)).collect::<Result<Vec<_>>>()?;
                let mut order: Vec<usize> = (0..inputs.len()).collect();
                order.sort_by_key(|&i| ids[i]);
                inputs = order.iter().map(|&i| inputs[i]).collect();
                let sources = order.iter().map(|&i| edges.get(&(id.clone(), ids[i]))).collect::<Vec<_>>();
                if sources.iter().any(Option::is_none) {
                    return value(format!("Unconnected input in {id}"));
                }
                let arguments: Option<Vec<usize>> =
                    sources.iter().map(|s| values.iter().find(|(port, _)| Some(port) == *s).map(|(_, n)| *n)).collect();
                let Some(arguments) = arguments else {
                    index += 1;
                    continue;
                };
                for (&argument, &port) in arguments.iter().zip(&inputs) {
                    if self.graph.shape(argument) != shape(port)? {
                        return value(format!("Input shape mismatch at {id}"));
                    }
                }
                let outputs = find_all(node, "output", "port");
                if outputs.len() != 1 {
                    return value("Only single-output legacy layers are supported");
                }
                let dims = shape(outputs[0])?;
                let attributes = children(node, "data")
                    .next()
                    .map(|data| data.attributes().map(|a| (a.name().to_owned(), a.value().to_owned())).collect())
                    .unwrap_or_default();
                let layer = Layer {
                    node,
                    id: id.clone(),
                    name: node.attribute("name").unwrap_or_default().to_owned(),
                    attributes,
                };
                let built = self.layer(&layer, &arguments, &dims)?;
                if self.graph.shape(built) != dims {
                    return value(format!(
                        "Output shape mismatch at {}: {} != {}",
                        layer.id,
                        repr(self.graph.shape(built)),
                        repr(&dims)
                    ));
                }
                self.graph.nodes[built].name.clone_from(&layer.name);
                values.push(((id, port_id(outputs[0])?), built));
                pending.remove(index);
                progressed = true;
            }
            if !progressed {
                return value("Cyclic graph or missing edge source");
            }
        }
        for (port, node) in values {
            if !consumed.contains(&port) {
                let name = format!("{}/result", self.graph.nodes[node].name);
                let shape = self.graph.shape(node).to_vec();
                let element = self.graph.nodes[node].element;
                self.graph.add(name, Op::Result, vec![node], shape, element);
            }
        }
        Ok(())
    }

    /// The operations of one legacy layer; returns the node holding the layer's output.
    fn layer(&mut self, layer: &Layer<'_, '_>, args: &[usize], dims: &[i64]) -> Result<usize> {
        let kind = layer.node.attribute("type").unwrap_or_default();
        let weights = self.weights;
        let graph = &mut self.graph;
        let input = |i: usize| args.get(i).copied().map_or_else(|| value("Missing layer input"), Ok);
        Ok(match (kind, layer.get("operation"), layer.get("type")) {
            ("Input", ..) => {
                if layer.node.attribute("precision") != Some("FP32") {
                    return value("Expected FP32 input");
                }
                graph.add(layer.name.clone(), Op::Parameter, Vec::new(), dims.to_vec(), Element::F32)
            }
            ("Const", ..) => {
                let element = match layer.node.attribute("precision") {
                    Some("I32") => Element::I32,
                    Some("I64") => Element::I64,
                    Some("FP32") => Element::F32,
                    _ => return value("Unsupported constant precision"),
                };
                let data = blob(weights, layer, "custom", dims, element)?;
                graph.constant(layer.name.clone(), element, dims.to_vec(), data)
            }
            ("Convolution", ..) => self.convolution(layer, input(0)?)?,
            ("ReLU", ..) => {
                let slope: f64 = layer
                    .get("negative_slope")
                    .unwrap_or("0")
                    .trim()
                    .parse()
                    .or_else(|_| value("Invalid negative_slope"))?;
                let data = input(0)?;
                let shape = graph.shape(data).to_vec();
                if slope == 0.0 {
                    graph.add(layer.part("relu"), Op::Relu, vec![data], shape, Element::F32)
                } else {
                    let slope = graph.constant(
                        layer.part("slope"),
                        Element::F32,
                        Vec::new(),
                        (slope as f32).to_le_bytes().to_vec(),
                    );
                    graph.add(layer.part("prelu"), Op::PRelu, vec![data, slope], shape, Element::F32)
                }
            }
            ("PReLU", ..) => {
                let data = input(0)?;
                let count = if layer.get("channel_shared") == Some("1") { 1 } else { channels(graph.shape(data))? };
                let slope = blob(weights, layer, "weights", &[count], Element::F32)?;
                let slope = graph.constant(layer.part("slope"), Element::F32, vec![count], slope);
                let shape = graph.shape(data).to_vec();
                graph.add(layer.part("prelu"), Op::PRelu, vec![data, slope], shape, Element::F32)
            }
            ("Pooling", ..) => {
                let window = layer.window()?;
                let kernel = layer.ints("kernel", None)?;
                let ceil = match layer.get("rounding_type").unwrap_or("floor") {
                    "floor" => false,
                    "ceil" => true,
                    other => return value(format!("Unknown rounding type {other}")),
                };
                let average = match layer.required("pool-method")? {
                    "max" => None,
                    "avg" => Some(layer.get("exclude-pad") == Some("true")),
                    _ => return value("Unknown pooling method"),
                };
                let data = input(0)?;
                let input_shape = graph.shape(data).to_vec();
                let mut shape = input_shape[..2.min(input_shape.len())].to_vec();
                shape.extend(spatial(&input_shape, &window, &kernel, ceil)?);
                graph.add(
                    layer.part("pool"),
                    Op::Pool { window, kernel, ceil, average },
                    vec![data],
                    shape,
                    Element::F32,
                )
            }
            ("Reshape", ..) => {
                let data = input(0)?;
                let pattern = if args.len() == 2 {
                    args[1]
                } else {
                    graph.int64s(layer.part("shape"), &layer.ints("dim", None)?)
                };
                let Some(values) = graph.constant_values(pattern) else {
                    return value(format!(
                        "Unsupported operation: Reshape with a computed shape {}",
                        layer.attributes_repr()
                    ));
                };
                let shape = reshape(graph.shape(data), &values)?;
                graph.add(layer.part("reshape"), Op::Reshape, vec![data, pattern], shape, Element::F32)
            }
            ("FullyConnected", ..) => self.fully_connected(layer, input(0)?)?,
            ("SoftMax", ..) => {
                let data = input(0)?;
                let index: i64 = int(Some(layer.required("axis")?))?;
                axis(index, graph.shape(data).len())?;
                let shape = graph.shape(data).to_vec();
                graph.add(layer.part("softmax"), Op::Softmax { axis: index }, vec![data], shape, Element::F32)
            }
            ("Permute", ..) => {
                let data = input(0)?;
                let order = layer.ints("order", None)?;
                let input_shape = graph.shape(data).to_vec();
                let shape =
                    order.iter().map(|&a| axis(a, input_shape.len()).map(|a| input_shape[a])).collect::<Result<_>>()?;
                let order = graph.int64s(layer.part("order"), &order);
                graph.add(layer.part("transpose"), Op::Transpose, vec![data, order], shape, Element::F32)
            }
            ("Concat", ..) => {
                let index: i64 = int(Some(layer.required("axis")?))?;
                let first = graph.shape(input(0)?).to_vec();
                let joined = axis(index, first.len())?;
                let mut shape = first;
                shape[joined] = 0;
                for &arg in args {
                    let other = graph.shape(arg);
                    let compatible =
                        other.len() == shape.len() && (0..shape.len()).all(|i| i == joined || other[i] == shape[i]);
                    if !compatible {
                        return value("Concat inputs disagree in shape");
                    }
                    shape[joined] += other[joined];
                }
                graph.add(layer.part("concat"), Op::Concat { axis: index }, args.to_vec(), shape, Element::F32)
            }
            ("Eltwise", Some("sum"), _) => {
                let mut node = input(0)?;
                for (i, &arg) in args.iter().enumerate().skip(1) {
                    node = graph.add_op(layer.part(&format!("sum{i}")), node, arg)?;
                }
                node
            }
            ("Activation", _, Some("sigmoid")) => {
                let data = input(0)?;
                let shape = graph.shape(data).to_vec();
                graph.add(layer.part("sigmoid"), Op::Sigmoid, vec![data], shape, Element::F32)
            }
            _ => return value(format!("Unsupported operation: {kind} {}", layer.attributes_repr())),
        })
    }

    fn convolution(&mut self, layer: &Layer<'_, '_>, data: usize) -> Result<usize> {
        let kernel = layer.ints("kernel", None)?;
        let group = int(Some(layer.get("group").unwrap_or("1")))?;
        let channels = channels(self.graph.shape(data))?;
        let out_channels = int(Some(layer.required("output")?))?;
        if group <= 0 || channels % group != 0 || out_channels % group != 0 {
            return value("Invalid convolution groups");
        }
        let mut dims = vec![out_channels, channels / group];
        dims.extend(&kernel);
        let weights = blob(self.weights, layer, "weights", &dims, Element::F32)?;
        let window = layer.window()?;
        let dilations = layer.ints("dilations", Some("1,1"))?;
        let grouped = group != 1;
        if grouped {
            dims = [group, out_channels / group, channels / group].into_iter().chain(kernel.iter().copied()).collect();
        }
        let extents: Vec<i64> = kernel.iter().zip(&dilations).map(|(k, d)| (k - 1) * d + 1).collect();
        let input_shape = self.graph.shape(data).to_vec();
        if kernel.len() != dilations.len() {
            return value("Window attributes do not match the input rank");
        }
        let mut shape = vec![input_shape[0], out_channels];
        shape.extend(spatial(&input_shape, &window, &extents, false)?);
        let weights = self.graph.constant(layer.part("weights"), Element::F32, dims, weights);
        let op = Op::Convolution { window, dilations, grouped };
        let mut node = self.graph.add(layer.part("convolution"), op, vec![data, weights], shape, Element::F32);
        if Self::has_blob(layer, "biases") {
            let biases = blob(self.weights, layer, "biases", &[1, out_channels, 1, 1], Element::F32)?;
            let biases = self.graph.constant(layer.part("biases"), Element::F32, vec![1, out_channels, 1, 1], biases);
            node = self.graph.add_op(layer.part("bias"), node, biases)?;
        }
        Ok(node)
    }

    fn fully_connected(&mut self, layer: &Layer<'_, '_>, data: usize) -> Result<usize> {
        let size: f64 = layer.required("out-size")?.trim().parse().or_else(|_| value("Invalid out-size"))?;
        let out_size = size.trunc() as i64;
        let input_shape = self.graph.shape(data).to_vec();
        let features: i64 = input_shape.iter().skip(1).product();
        let pattern = self.graph.int64s(layer.part("flatten/shape"), &[0, -1]);
        let flat_shape = reshape(&input_shape, &[0, -1])?;
        let flattened =
            self.graph.add(layer.part("flatten"), Op::Reshape, vec![data, pattern], flat_shape, Element::F32);
        let weights = blob(self.weights, layer, "weights", &[out_size, features], Element::F32)?;
        let weights = self.graph.constant(layer.part("weights"), Element::F32, vec![out_size, features], weights);
        let shape = vec![self.graph.shape(flattened)[0], out_size];
        let mut node = self.graph.add(layer.part("matmul"), Op::MatMul, vec![flattened, weights], shape, Element::F32);
        if Self::has_blob(layer, "biases") {
            let biases = blob(self.weights, layer, "biases", &[out_size], Element::F32)?;
            let biases = self.graph.constant(layer.part("biases"), Element::F32, vec![out_size], biases);
            node = self.graph.add_op(layer.part("bias"), node, biases)?;
        }
        Ok(node)
    }
}

fn channels(shape: &[i64]) -> Result<i64> {
    shape.get(1).copied().map_or_else(|| value("Expected a channel dimension"), Ok)
}

#[cfg(test)]
mod tests;
