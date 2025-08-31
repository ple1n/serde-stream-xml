// RustyXML
// Copyright 2013-2016 RustyXML developers
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use serde::de::{IgnoredAny, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

use crate::element_builder::{BuilderError, ElementBuilder};
use crate::parser::{Parser, Pos};
use crate::{escape, AttrMap, Xml};

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::iter::IntoIterator;
use std::slice;
use std::str::FromStr;
use tracing::info;

#[derive(Clone, PartialEq, Debug)]
/// A struct representing an XML element
pub struct Element {
    /// The element's name
    pub name: String,
    /// The element's namespace
    pub ns: Option<String>,
    /// The element's attributes
    pub attributes: AttrMap<(String, Option<String>), String>,
    /// The element's child `Xml` nodes
    pub children: Vec<Xml>,
    /// The prefixes set for known namespaces
    pub(crate) prefixes: HashMap<String, String>,
    /// The element's default namespace
    pub(crate) default_ns: Option<String>,
}

/// to handle repeated entries
pub fn map_collect<K: Hash + Eq, V>(map: &mut HashMap<K, Vec<V>>, k: K, val: V) {
    if let Some(e) = map.get_mut(&k) {
        e.push(val);
    } else {
        map.insert(k, [val].into());
    }
}

impl<'de> Deserialize<'de> for Element {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer
            .deserialize_ignored_any(IgnoredAny)
            .map(|x| Element::new("todo".to_owned(), None, vec![]))
    }
}

/// Produces one or more entries
pub fn type_guess<S: serde::Serializer>(
    key: &str,
    val: &str,
    map: &mut S::SerializeMap,
) -> Result<(), S::Error> {
    // Try parsing as f32, for examples like "0", "0.1", ".0"
    if let Ok(value) = val.parse::<bool>() {
        map.serialize_entry(key, &value)?;
        return Ok(());
    }
    if let Ok(value) = val.parse::<u64>() {
        map.serialize_entry(key, &value)?;
        return Ok(());
    }
    if let Ok(value) = val.parse::<f32>() {
        map.serialize_entry(key, &value)?;
        return Ok(());
    }

    // Try parsing as two fields, for input like "200 MG" "200 mg" "100 mg/1" "20 MG/ML"
    let parts: Vec<&str> = val.split_whitespace().collect();
    if parts.len() == 2 {
        if let Ok(n) = parts[0].parse::<f32>() {
            let denom = parts[1].to_lowercase();
            map.serialize_entry(key, &n)?;
            map.serialize_entry(&format!("{}_unit", key), &denom)?;
            return Ok(());
        }
    } else {
        info!("parts {:?}", &parts);
    }
    info!("{}, {}", key, val);
    map.serialize_entry(key, val)
}

pub fn type_guess_val<S: serde::Serializer>(val: &str, s: S) -> Result<S::Ok, S::Error> {
    // Try parsing as f32, for examples like "0", "0.1", ".0"
    if let Ok(value) = val.parse::<u64>() {
        s.serialize_u64(value)
    } else if let Ok(value) = val.parse::<bool>() {
        s.serialize_bool(value)
    } else if let Ok(value) = val.parse::<f32>() {
        s.serialize_f32(value)
    } else {
        s.serialize_str(val)
    }
}

// All entries are handled like key:[val]
// Elasticsearch style
impl Serialize for Element {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        /*
           element.name: {
               ..atttrs,
               ..children
           }
        */

        if self.attributes.len() == 0 && self.children.len() == 0 {
            return serializer.serialize_unit();
        }
        let attr_num = self.attributes.len() + self.children.len();

        let mut elements = HashMap::new();
        let mut comments = Vec::new();
        let mut texts = Vec::new();
        for kid in &self.children {
            match kid {
                Xml::ElementNode(el) => map_collect(&mut elements, el.name.clone(), el),
                Xml::CommentNode(c) => comments.push(c),
                Xml::CharacterNode(text) => {
                    let t = text.trim();
                    if !t.is_empty() {
                        texts.push(t)
                    }
                }
                _ => continue, // unsound, too lazy
            };
        }
        if elements.len() == 0 && comments.len() == 0 && self.attributes.len() == 0 {
            if texts.len() == 1 {
                type_guess_val(&texts[0], serializer)
            } else {
                texts.serialize(serializer)
            }
        } else {
            let mut mapper = serializer.serialize_map(Some(attr_num))?;
            for ((key, _no_idea), val) in &self.attributes {
                type_guess::<S>(&key, &val, &mut mapper)?;
            }

            for (key, vec) in elements {
                match vec.len() {
                    0 => (),
                    1 => mapper.serialize_entry(&key, &vec[0])?,
                    _ => mapper.serialize_entry(&key, &vec)?,
                };
            }
            match comments.len() {
                0 => (),
                1 => mapper.serialize_entry("_comment", &comments[0])?,
                _ => mapper.serialize_entry("_comment", &comments)?,
            };
            match texts.len() {
                0 => (),
                1 => mapper.serialize_entry("_body", &texts[0])?,
                _ => mapper.serialize_entry("_body", &texts)?,
            };
            mapper.end()
        }
    }
}

fn fmt_elem(
    elem: &Element,
    parent: Option<&Element>,
    all_prefixes: &HashMap<String, String>,
    f: &mut fmt::Formatter,
) -> fmt::Result {
    let mut all_prefixes = all_prefixes.clone();
    all_prefixes.extend(elem.prefixes.clone().into_iter());

    // Do we need a prefix?
    if elem.ns != elem.default_ns {
        let prefix = all_prefixes
            .get(elem.ns.as_ref().map_or("", |x| &x[..]))
            .expect("No namespace prefix bound");
        write!(f, "<{}:{}", *prefix, elem.name)?;
    } else {
        write!(f, "<{}", elem.name)?;
    }

    // Do we need to set the default namespace ?
    if !elem
        .attributes
        .iter()
        .any(|(&(ref name, _), _)| name == "xmlns")
    {
        match (parent, &elem.default_ns) {
            // No parent, namespace is not empty
            (None, &Some(ref ns)) => write!(f, " xmlns='{}'", *ns)?,
            // Parent and child namespace differ
            (Some(parent), ns) if parent.default_ns != *ns => {
                write!(f, " xmlns='{}'", ns.as_ref().map_or("", |x| &x[..]))?
            }
            _ => (),
        }
    }

    for (&(ref name, ref ns), value) in &elem.attributes {
        match *ns {
            Some(ref ns) => {
                let prefix = all_prefixes.get(ns).expect("No namespace prefix bound");
                write!(f, " {}:{}='{}'", *prefix, name, escape(value))?
            }
            None => write!(f, " {}='{}'", name, escape(value))?,
        }
    }

    if elem.children.is_empty() {
        write!(f, "/>")?;
    } else {
        write!(f, ">")?;
        for child in &elem.children {
            match *child {
                Xml::ElementNode(ref child) => fmt_elem(child, Some(elem), &all_prefixes, f)?,
                ref o => fmt::Display::fmt(o, f)?,
            }
        }
        if elem.ns != elem.default_ns {
            let prefix = all_prefixes
                .get(elem.ns.as_ref().unwrap())
                .expect("No namespace prefix bound");
            write!(f, "</{}:{}>", *prefix, elem.name)?;
        } else {
            write!(f, "</{}>", elem.name)?;
        }
    }

    Ok(())
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt_elem(self, None, &HashMap::new(), f)
    }
}

/// An iterator returning filtered child `Element`s of another `Element`
pub struct ChildElements<'a, 'b> {
    elems: slice::Iter<'a, Xml>,
    name: &'b str,
    ns: Option<&'b str>,
}

impl<'a, 'b> Iterator for ChildElements<'a, 'b> {
    type Item = &'a Element;

    fn next(&mut self) -> Option<&'a Element> {
        let (name, ns) = (self.name, self.ns);
        self.elems.by_ref().find_map(|child| {
            if let Xml::ElementNode(ref elem) = *child {
                if name == elem.name && ns == elem.ns.as_ref().map(|x| &x[..]) {
                    return Some(elem);
                }
            }
            None
        })
    }
}

impl Element {
    /// Create a new `Element`, with specified name and namespace.
    /// Attributes are specified as a `Vec` of `(name, namespace, value)` tuples.
    pub fn new<A>(name: String, ns: Option<String>, attrs: A) -> Element
    where
        A: IntoIterator<Item = (String, Option<String>, String)>,
    {
        let mut prefixes = HashMap::with_capacity(2);
        prefixes.insert(
            "http://www.w3.org/XML/1998/namespace".to_owned(),
            "xml".to_owned(),
        );
        prefixes.insert(
            "http://www.w3.org/2000/xmlns/".to_owned(),
            "xmlns".to_owned(),
        );

        let attributes: AttrMap<_, _> = attrs
            .into_iter()
            .map(|(name, ns, value)| ((name, ns), value))
            .collect();

        Element {
            name,
            ns: ns.clone(),
            default_ns: ns,
            prefixes,
            attributes,
            children: Vec::new(),
        }
    }

    /// Returns the character and CDATA contained in the element.
    pub fn content_str(&self) -> String {
        let mut res = String::new();
        for child in &self.children {
            match *child {
                Xml::ElementNode(ref elem) => res.push_str(&elem.content_str()),
                Xml::CharacterNode(ref data) | Xml::CDATANode(ref data) => res.push_str(data),
                _ => (),
            }
        }
        res
    }

    /// Gets an attribute with the specified name and namespace. When an attribute with the
    /// specified name does not exist `None` is returned.
    pub fn get_attribute<'a>(&'a self, name: &str, ns: Option<&str>) -> Option<&'a str> {
        self.attributes
            .get(&(name.to_owned(), ns.map(|x| x.to_owned())))
            .map(|x| &x[..])
    }

    /// Sets the attribute with the specified name and namespace.
    /// Returns the original value.
    pub fn set_attribute(
        &mut self,
        name: String,
        ns: Option<String>,
        value: String,
    ) -> Option<String> {
        self.attributes.insert((name, ns), value)
    }

    /// Remove the attribute with the specified name and namespace.
    /// Returns the original value.
    pub fn remove_attribute(&mut self, name: &str, ns: Option<&str>) -> Option<String> {
        self.attributes
            .remove(&(name.to_owned(), ns.map(|x| x.to_owned())))
    }

    /// Gets the first child `Element` with the specified name and namespace. When no child
    /// with the specified name exists `None` is returned.
    pub fn get_child<'a>(&'a self, name: &str, ns: Option<&str>) -> Option<&'a Element> {
        self.get_children(name, ns).next()
    }

    /// Get all children `Element` with the specified name and namespace. When no child
    /// with the specified name exists an empty vetor is returned.
    pub fn get_children<'a, 'b>(
        &'a self,
        name: &'b str,
        ns: Option<&'b str>,
    ) -> ChildElements<'a, 'b> {
        ChildElements {
            elems: self.children.iter(),
            name,
            ns,
        }
    }

    /// Appends a child element. Returns a reference to the added element.
    pub fn tag(&mut self, child: Element) -> &mut Element {
        self.children.push(Xml::ElementNode(child));
        match self.children.last_mut() {
            Some(Xml::ElementNode(ref mut elem)) => elem,
            _ => unreachable!("Could not get reference to just added element!"),
        }
    }

    /// Appends a child element. Returns a mutable reference to self.
    pub fn tag_stay(&mut self, child: Element) -> &mut Element {
        self.children.push(Xml::ElementNode(child));
        self
    }

    /// Appends characters. Returns a mutable reference to self.
    pub fn text(&mut self, text: String) -> &mut Element {
        self.children.push(Xml::CharacterNode(text));
        self
    }

    /// Appends CDATA. Returns a mutable reference to self.
    pub fn cdata(&mut self, text: String) -> &mut Element {
        self.children.push(Xml::CDATANode(text));
        self
    }

    /// Appends a comment. Returns a mutable reference to self.
    pub fn comment(&mut self, text: String) -> &mut Element {
        self.children.push(Xml::CommentNode(text));
        self
    }

    /// Appends processing information. Returns a mutable reference to self.
    pub fn pi(&mut self, text: String) -> &mut Element {
        self.children.push(Xml::PINode(text));
        self
    }
}

impl FromStr for Element {
    type Err = BuilderError;
    #[inline]
    fn from_str(data: &str) -> Result<Element, BuilderError> {
        todo!();

        let mut p = Parser::new();
        let mut e = ElementBuilder::new();

        p.feed_str(data);
        // TODO: Panics
        p.find_map(|x| e.handle_event(x.unwrap().0))
            .unwrap_or(Err(BuilderError::NoElement))
    }
}

#[cfg(test)]
mod tests {
    use super::Element;
    use serde::ser::{SerializeMap, Serializer};
    use std::collections::HashMap;

    #[test]
    fn test_get_children() {
        let elem: Element = "<a><b/><c/><b/></a>".parse().unwrap();
        assert_eq!(
            elem.get_children("b", None).collect::<Vec<_>>(),
            vec![
                &Element::new("b".to_owned(), None, vec![]),
                &Element::new("b".to_owned(), None, vec![])
            ],
        );
    }

    #[test]
    fn test_get_child() {
        let elem: Element = "<a><b/><c/><b/></a>".parse().unwrap();
        assert_eq!(
            elem.get_child("b", None),
            Some(&Element::new("b".to_owned(), None, vec![])),
        );
    }

    #[test]
    #[cfg(feature = "ordered_attrs")]
    fn test_attribute_order_new() {
        let input_attributes = vec![
            ("href".to_owned(), None, "/".to_owned()),
            ("title".to_owned(), None, "Home".to_owned()),
            ("target".to_owned(), None, "_blank".to_owned()),
        ];

        // Run this 5 times to make it unlikely this test succeeds at random
        for _ in 0..5 {
            let elem = Element::new("a".to_owned(), None, input_attributes.clone());
            for (expected, actual) in input_attributes.iter().zip(elem.attributes) {
                assert_eq!(expected.0, (actual.0).0);
                assert_eq!(expected.1, (actual.0).1);
                assert_eq!(expected.2, actual.1);
            }
        }
    }

    #[test]
    #[cfg(feature = "ordered_attrs")]
    fn test_attribute_order_added() {
        let input_attributes = vec![
            ("href".to_owned(), None, "/".to_owned()),
            ("title".to_owned(), None, "Home".to_owned()),
            ("target".to_owned(), None, "_blank".to_owned()),
        ];

        // Run this 5 times to make it unlikely this test succeeds at random
        for _ in 0..5 {
            let mut elem = Element::new("a".to_owned(), None, vec![]);
            for attr in &input_attributes {
                elem.set_attribute(attr.0.clone(), attr.1.clone(), attr.2.clone());
            }
            for (expected, actual) in input_attributes.iter().zip(elem.attributes) {
                assert_eq!(expected.0, (actual.0).0);
                assert_eq!(expected.1, (actual.0).1);
                assert_eq!(expected.2, actual.1);
            }
        }
    }

    #[test]
    fn test_type_guess_number() {
        let mut result = HashMap::new();
        let mut map = serde_test::MapSerializer::new(&mut result);

        super::type_guess("dose", "42.5", &mut map).unwrap();

        assert_eq!(result.get("dose"), Some(&serde_test::Token::F32(42.5)));
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_type_guess_number_unit() {
        let mut result = HashMap::new();
        let mut map = serde_test::MapSerializer::new(&mut result);

        super::type_guess("dose", "200 MG", &mut map).unwrap();

        assert_eq!(result.get("dose"), Some(&serde_test::Token::F32(200.0)));
        assert_eq!(result.get("dose_unit"), Some(&serde_test::Token::Str("mg")));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_type_guess_string_fallback() {
        let mut result = HashMap::new();
        let mut map = serde_test::MapSerializer::new(&mut result);

        super::type_guess("name", "Aspirin", &mut map).unwrap();

        assert_eq!(result.get("name"), Some(&serde_test::Token::Str("Aspirin")));
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_type_guess_mixed_case_unit() {
        let mut result = HashMap::new();
        let mut map = serde_test::MapSerializer::new(&mut result);

        super::type_guess("dose", "100 Mg", &mut map).unwrap();

        assert_eq!(result.get("dose"), Some(&serde_test::Token::F32(100.0)));
        assert_eq!(result.get("dose_unit"), Some(&serde_test::Token::Str("mg")));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_type_guess_invalid_number_unit() {
        let mut result = HashMap::new();
        let mut map = serde_test::MapSerializer::new(&mut result);

        super::type_guess("dose", "abc MG", &mut map).unwrap();

        assert_eq!(result.get("dose"), Some(&serde_test::Token::Str("abc MG")));
        assert_eq!(result.len(), 1);
    }
}
