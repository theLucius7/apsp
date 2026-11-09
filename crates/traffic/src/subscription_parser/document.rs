use super::{MAX_BODY_BYTES, MAX_DEPTH, MAX_SCALAR_BYTES, MAX_VALUES, ParseError};
use saphyr_parser::{Event, Parser, ScalarStyle};
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::{collections::BTreeMap, fmt, rc::Rc};

#[derive(Default)]
struct Budget {
    values: usize,
    bytes: usize,
}

impl Budget {
    fn charge(&mut self, values: usize, bytes: usize) -> Result<(), ParseError> {
        self.values = self
            .values
            .checked_add(values)
            .ok_or_else(ParseError::limit)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(ParseError::limit)?;
        if self.values > MAX_VALUES || self.bytes > MAX_BODY_BYTES {
            return Err(ParseError::limit());
        }
        Ok(())
    }

    fn scalar(&mut self, bytes: usize) -> Result<(), ParseError> {
        if bytes > MAX_SCALAR_BYTES {
            return Err(ParseError::limit());
        }
        self.charge(1, bytes)
    }
}

struct JsonSeed<'a> {
    budget: &'a mut Budget,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.depth > MAX_DEPTH {
            return Err(D::Error::custom("subscription-limit"));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for JsonSeed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded subscription document")
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        self.budget
            .scalar(5)
            .map_err(|_| E::custom("subscription-limit"))?;
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        self.budget
            .scalar(20)
            .map_err(|_| E::custom("subscription-limit"))?;
        Ok(value.into())
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        self.budget
            .scalar(20)
            .map_err(|_| E::custom("subscription-limit"))?;
        Ok(value.into())
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        self.budget
            .scalar(24)
            .map_err(|_| E::custom("subscription-limit"))?;
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("subscription-invalid"))
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        self.budget
            .scalar(4)
            .map_err(|_| E::custom("subscription-limit"))?;
        Ok(Value::Null)
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        self.budget
            .scalar(value.len())
            .map_err(|_| E::custom("subscription-limit"))?;
        Ok(Value::String(value.into()))
    }

    fn visit_string<E: Error>(self, value: String) -> Result<Value, E> {
        self.budget
            .scalar(value.len())
            .map_err(|_| E::custom("subscription-limit"))?;
        Ok(Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        self.budget
            .charge(1, 0)
            .map_err(|_| A::Error::custom("subscription-limit"))?;
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonSeed {
            budget: self.budget,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut mapping: A) -> Result<Value, A::Error> {
        self.budget
            .charge(1, 0)
            .map_err(|_| A::Error::custom("subscription-limit"))?;
        let mut values = Map::new();
        while let Some(key) = mapping.next_key::<String>()? {
            self.budget
                .scalar(key.len())
                .map_err(|_| A::Error::custom("subscription-limit"))?;
            if values.contains_key(&key) {
                return Err(A::Error::custom("subscription-duplicate"));
            }
            let value = mapping.next_value_seed(JsonSeed {
                budget: self.budget,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

pub(super) fn json(text: &str) -> Result<Value, ParseError> {
    let mut budget = Budget::default();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = JsonSeed {
        budget: &mut budget,
        depth: 1,
    }
    .deserialize(&mut deserializer)
    .map_err(json_error)?;
    deserializer.end().map_err(json_error)?;
    Ok(value)
}

fn json_error(error: serde_json::Error) -> ParseError {
    // The deserializer's original message may contain configuration secrets.
    let internal = error.to_string();
    if internal.contains("subscription-limit") {
        ParseError::limit()
    } else if internal.contains("subscription-duplicate") {
        duplicate()
    } else {
        ParseError::document()
    }
}

fn duplicate() -> ParseError {
    ParseError::new("duplicate_field", "订阅内容存在重复字段")
}

enum YamlValue {
    Scalar(Value),
    Sequence(Vec<Rc<YamlNode>>),
    Mapping(BTreeMap<String, Rc<YamlNode>>),
}

struct YamlNode {
    value: YamlValue,
    values: usize,
    bytes: usize,
    depth: usize,
}

enum FrameValue {
    Sequence(Vec<Rc<YamlNode>>),
    Mapping {
        entries: BTreeMap<String, Rc<YamlNode>>,
        key: Option<String>,
        merge: Option<Rc<YamlNode>>,
    },
}

struct Frame {
    anchor: usize,
    value: FrameValue,
}

fn attach(
    node: Rc<YamlNode>,
    frames: &mut [Frame],
    root: &mut Option<Rc<YamlNode>>,
) -> Result<(), ParseError> {
    match frames.last_mut().map(|frame| &mut frame.value) {
        Some(FrameValue::Sequence(values)) => values.push(node),
        Some(FrameValue::Mapping {
            entries,
            key,
            merge,
        }) => {
            if let Some(name) = key.take() {
                if name == "<<" {
                    if merge.replace(node).is_some() {
                        return Err(duplicate());
                    }
                } else if entries.insert(name, node).is_some() {
                    return Err(duplicate());
                }
            } else {
                let YamlValue::Scalar(Value::String(name)) = &node.value else {
                    return Err(ParseError::document());
                };
                if entries.contains_key(name) || (name == "<<" && merge.is_some()) {
                    return Err(duplicate());
                }
                *key = Some(name.clone());
            }
        }
        None => {
            if root.replace(node).is_some() {
                return Err(ParseError::document());
            }
        }
    }
    Ok(())
}

fn merge_mapping(
    target: &mut BTreeMap<String, Rc<YamlNode>>,
    node: &YamlNode,
) -> Result<(), ParseError> {
    match &node.value {
        YamlValue::Mapping(entries) => {
            for (key, value) in entries {
                if target.insert(key.clone(), value.clone()).is_some() {
                    return Err(duplicate());
                }
            }
        }
        YamlValue::Sequence(nodes) => {
            for node in nodes {
                merge_mapping(target, node)?;
            }
        }
        YamlValue::Scalar(_) => return Err(ParseError::document()),
    }
    Ok(())
}

fn finish(frame: Frame) -> Result<Rc<YamlNode>, ParseError> {
    let value = match frame.value {
        FrameValue::Sequence(values) => YamlValue::Sequence(values),
        FrameValue::Mapping {
            entries,
            key,
            merge,
        } => {
            if key.is_some() {
                return Err(ParseError::document());
            }
            let mut merged = BTreeMap::new();
            if let Some(node) = merge {
                merge_mapping(&mut merged, &node)?;
            }
            // Explicit keys override an anchor template, never another explicit key.
            merged.extend(entries);
            YamlValue::Mapping(merged)
        }
    };
    let (mut values, mut bytes, mut depth) = (1usize, 0usize, 1usize);
    let mut add = |node: &YamlNode, key_bytes: usize| -> Result<(), ParseError> {
        values = values
            .checked_add(node.values + usize::from(key_bytes > 0))
            .ok_or_else(ParseError::limit)?;
        bytes = bytes
            .checked_add(node.bytes + key_bytes)
            .ok_or_else(ParseError::limit)?;
        depth = depth.max(node.depth + 1);
        if values > MAX_VALUES || bytes > MAX_BODY_BYTES || depth > MAX_DEPTH {
            return Err(ParseError::limit());
        }
        Ok(())
    };
    match &value {
        YamlValue::Sequence(nodes) => {
            for node in nodes {
                add(node, 0)?;
            }
        }
        YamlValue::Mapping(nodes) => {
            for (key, node) in nodes {
                add(node, key.len())?;
            }
        }
        YamlValue::Scalar(_) => return Err(ParseError::document()),
    }
    Ok(Rc::new(YamlNode {
        value,
        values,
        bytes,
        depth,
    }))
}

fn scalar(
    text: &str,
    style: ScalarStyle,
    tag: Option<&saphyr_parser::Tag>,
) -> Result<Value, ParseError> {
    let explicit = if let Some(tag) = tag {
        if !tag.is_yaml_core_schema() {
            return Err(ParseError::document());
        }
        Some(tag.suffix.as_str())
    } else {
        None
    };
    if explicit == Some("str") || (explicit.is_none() && style != ScalarStyle::Plain) {
        return Ok(Value::String(text.into()));
    }
    let value = match text {
        "" | "null" | "Null" | "NULL" | "~" => Value::Null,
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        ".nan" | ".NaN" | ".NAN" | ".inf" | ".Inf" | ".INF" | "-.inf" | "+.inf" => {
            return Err(ParseError::document());
        }
        _ => {
            let number = text
                .strip_prefix("0x")
                .and_then(|n| u64::from_str_radix(n, 16).ok())
                .or_else(|| {
                    text.strip_prefix("0o")
                        .and_then(|n| u64::from_str_radix(n, 8).ok())
                });
            if let Some(number) = number {
                Value::Number(number.into())
            } else if let Ok(number) = text.parse::<i64>() {
                Value::Number(number.into())
            } else if let Ok(number) = text.parse::<u64>() {
                Value::Number(number.into())
            } else if let Ok(number) = text.parse::<f64>() {
                Value::Number(Number::from_f64(number).ok_or_else(ParseError::document)?)
            } else {
                Value::String(text.into())
            }
        }
    };
    match explicit {
        None => Ok(value),
        Some("null") if value.is_null() => Ok(value),
        Some("bool") if value.is_boolean() => Ok(value),
        Some("int") if value.as_i64().is_some() || value.as_u64().is_some() => Ok(value),
        Some("float") if value.is_number() => Ok(value),
        _ => Err(ParseError::document()),
    }
}

fn expand(node: &YamlNode) -> Value {
    match &node.value {
        YamlValue::Scalar(value) => value.clone(),
        YamlValue::Sequence(values) => {
            Value::Array(values.iter().map(|value| expand(value)).collect())
        }
        YamlValue::Mapping(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), expand(value)))
                .collect(),
        ),
    }
}

pub(super) fn yaml(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser::new_from_str(text);
    let mut budget = Budget::default();
    let mut frames = Vec::<Frame>::new();
    let mut anchors = BTreeMap::<usize, Rc<YamlNode>>::new();
    let mut root = None;
    let (mut documents, mut aliases, mut anchor_count, mut events) =
        (0usize, 0usize, 0usize, 0usize);
    while let Some(event) = parser.next_event() {
        let (event, _) = event.map_err(|_| ParseError::document())?;
        let is_sequence_start = matches!(&event, Event::SequenceStart(..));
        events += 1;
        if events > MAX_VALUES * 3 {
            return Err(ParseError::limit());
        }
        match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd => {}
            Event::DocumentStart(_) => {
                documents += 1;
                if documents != 1 {
                    return Err(ParseError::document());
                }
            }
            Event::DocumentEnd => {
                if !frames.is_empty() {
                    return Err(ParseError::document());
                }
            }
            Event::SequenceStart(anchor, tag) | Event::MappingStart(anchor, tag) => {
                if tag.is_some() {
                    return Err(ParseError::document());
                }
                if frames.len() + 1 > MAX_DEPTH {
                    return Err(ParseError::limit());
                }
                budget.charge(1, 0)?;
                if anchor != 0 {
                    anchor_count += 1;
                }
                if anchor_count > 1024 {
                    return Err(ParseError::limit());
                }
                let value = if is_sequence_start {
                    FrameValue::Sequence(Vec::new())
                } else {
                    FrameValue::Mapping {
                        entries: BTreeMap::new(),
                        key: None,
                        merge: None,
                    }
                };
                frames.push(Frame { anchor, value });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                let frame = frames.pop().ok_or_else(ParseError::document)?;
                let anchor = frame.anchor;
                let node = finish(frame)?;
                if anchor != 0 {
                    anchors.insert(anchor, node.clone());
                }
                attach(node, &mut frames, &mut root)?;
            }
            Event::Scalar(text, style, anchor, tag) => {
                budget.scalar(text.len())?;
                if frames.len() + 1 > MAX_DEPTH {
                    return Err(ParseError::limit());
                }
                let value = scalar(&text, style, tag.as_deref())?;
                let node = Rc::new(YamlNode {
                    value: YamlValue::Scalar(value),
                    values: 1,
                    bytes: text.len(),
                    depth: 1,
                });
                if anchor != 0 {
                    anchor_count += 1;
                    if anchor_count > 1024 {
                        return Err(ParseError::limit());
                    }
                    anchors.insert(anchor, node.clone());
                }
                attach(node, &mut frames, &mut root)?;
            }
            Event::Alias(anchor) => {
                aliases += 1;
                if aliases > 10_000 {
                    return Err(ParseError::limit());
                }
                // Open anchors are absent: self references and recursive expansion are rejected.
                let node = anchors.get(&anchor).ok_or_else(ParseError::document)?;
                if frames.len() + node.depth > MAX_DEPTH {
                    return Err(ParseError::limit());
                }
                budget.charge(node.values, node.bytes)?;
                attach(node.clone(), &mut frames, &mut root)?;
            }
        }
    }
    if documents != 1 || !frames.is_empty() {
        return Err(ParseError::document());
    }
    let root = root.ok_or_else(ParseError::document)?;
    Ok(expand(root.as_ref()))
}
