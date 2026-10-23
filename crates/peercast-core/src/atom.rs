use bytes::{Buf, BufMut, Bytes};
use std::fmt;
use std::net::IpAddr;
use thiserror::Error;

use crate::id4::ID4;
use crate::scalar::{Scalar, ScalarError};

/// Errors encountered when serializing or deserializing Atoms.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum AtomError {
    #[error("Leaf length {actual} exceeds maximum allowed {max}")]
    LeafLengthExceeded { actual: usize, max: usize },

    #[error("Child count {actual} exceeds maximum allowed {max}")]
    ChildCountExceeded { actual: usize, max: usize },

    #[error("Depth {actual} exceeds maximum allowed depth {max}")]
    DepthExceeded { actual: usize, max: usize },

    #[error("Total node count {actual} exceeds maximum allowed {max}")]
    TotalNodesExceeded { actual: usize, max: usize },

    #[error("Total tree byte size {actual} exceeds budget {max}")]
    TotalBytesExceeded { actual: usize, max: usize },

    #[error("Descriptor overflow: length/count {0} exceeds 31-bit limit")]
    DescriptorOverflow(usize),

    #[error("Unexpected EOF / truncated Atom data (needed {needed} bytes, had {available})")]
    Truncated { needed: usize, available: usize },

    #[error("Type mismatch: expected leaf atom, found parent atom {0:?}")]
    ExpectedLeaf(ID4),

    #[error("Type mismatch: expected parent atom, found leaf atom {0:?}")]
    ExpectedParent(ID4),

    #[error("Exact width mismatch: expected {expected} bytes, got {actual}")]
    WidthMismatch { expected: usize, actual: usize },

    #[error("Invalid IP address byte length: expected 4 or 16 bytes, got {0}")]
    InvalidIpLength(usize),

    #[error("String is missing required terminating NUL byte")]
    MissingTerminatingNul,

    #[error("Invalid UTF-8 string: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
}

impl From<ScalarError> for AtomError {
    fn from(err: ScalarError) -> Self {
        match err {
            ScalarError::WidthMismatch { expected, actual } => {
                AtomError::WidthMismatch { expected, actual }
            }
            ScalarError::InvalidIpLength(len) => AtomError::InvalidIpLength(len),
            ScalarError::MissingTerminatingNul => AtomError::MissingTerminatingNul,
            ScalarError::InvalidUtf8(e) => AtomError::InvalidUtf8(e),
        }
    }
}

/// Resource and depth limits for Atom parsing (REQ-025, REQ-026).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomBudget {
    /// Maximum direct child Atoms per parent (default: 1,024).
    pub max_children: usize,
    /// Maximum payload byte length for a single generic leaf Atom (default: 1 MiB = 1,048,576 bytes).
    pub max_leaf_len: usize,
    /// Maximum parent nesting depth (default: 32).
    pub max_depth: usize,
    /// Maximum total Atom nodes across the entire decoded tree (default: 4,096).
    pub max_total_nodes: usize,
    /// Maximum total serialized byte budget across the entire decoded tree (default: 4 MiB = 4,194,304 bytes).
    pub max_total_bytes: usize,
}

impl Default for AtomBudget {
    fn default() -> Self {
        Self {
            max_children: 1024,
            max_leaf_len: 1024 * 1024,       // 1 MiB
            max_depth: 32,
            max_total_nodes: 4096,
            max_total_bytes: 4 * 1024 * 1024, // 4 MiB
        }
    }
}

impl AtomBudget {
    /// Returns a budget tailored for media packets with a 16 KiB leaf payload cap (REQ-025/REQ-026).
    pub fn for_media_packet() -> Self {
        Self {
            max_children: 1024,
            max_leaf_len: 16 * 1024, // 16 KiB
            max_depth: 32,
            max_total_nodes: 4096,
            max_total_bytes: 4 * 1024 * 1024,
        }
    }

    /// Builder helper to adjust maximum leaf length.
    pub fn with_max_leaf_len(mut self, len: usize) -> Self {
        self.max_leaf_len = len;
        self
    }
}

/// A PCP binary Atom node representing either a Leaf or a Parent container (REQ-020).
///
/// On the wire:
/// - A Leaf has a 4-byte ID4 tag followed by a little-endian u32 byte length (bit 31 is 0),
///   followed by that many payload bytes.
/// - A Parent has a 4-byte ID4 tag followed by a little-endian u32 where bit 31 is 1
///   and bits 0..30 specify the direct child count, followed by each child Atom in order.
#[derive(Clone, PartialEq, Eq)]
pub enum Atom {
    Leaf { name: ID4, value: Bytes },
    Parent { name: ID4, children: Vec<Atom> },
}

impl Atom {
    // --- Constructors ---

    /// Creates a leaf Atom with raw payload bytes.
    pub fn new_leaf(name: ID4, value: impl Into<Bytes>) -> Self {
        Atom::Leaf {
            name,
            value: value.into(),
        }
    }

    /// Creates an empty leaf Atom (0 bytes payload).
    pub fn new_empty_leaf(name: ID4) -> Self {
        Atom::Leaf {
            name,
            value: Bytes::new(),
        }
    }

    /// Creates a parent container Atom with child Atoms.
    pub fn new_parent(name: ID4, children: Vec<Atom>) -> Self {
        Atom::Parent { name, children }
    }

    /// Creates an empty parent container Atom (0 children).
    pub fn new_empty_parent(name: ID4) -> Self {
        Atom::Parent {
            name,
            children: Vec::new(),
        }
    }

    /// Creates a leaf Atom containing a single u8 integer.
    pub fn new_u8(name: ID4, val: u8) -> Self {
        Atom::new_leaf(name, Bytes::copy_from_slice(&Scalar::write_u8(val)))
    }

    /// Creates a leaf Atom containing a 16-bit little-endian integer.
    pub fn new_u16_le(name: ID4, val: u16) -> Self {
        Atom::new_leaf(name, Bytes::copy_from_slice(&Scalar::write_u16_le(val)))
    }

    /// Creates a leaf Atom containing a 32-bit little-endian integer.
    pub fn new_u32_le(name: ID4, val: u32) -> Self {
        Atom::new_leaf(name, Bytes::copy_from_slice(&Scalar::write_u32_le(val)))
    }

    /// Creates a leaf Atom containing a 32-bit signed little-endian integer.
    pub fn new_i32_le(name: ID4, val: i32) -> Self {
        Atom::new_leaf(name, Bytes::copy_from_slice(&Scalar::write_u32_le(val as u32)))
    }

    /// Creates a leaf Atom containing a 64-bit little-endian integer.
    pub fn new_u64_le(name: ID4, val: u64) -> Self {
        Atom::new_leaf(name, Bytes::copy_from_slice(&Scalar::write_u64_le(val)))
    }

    /// Creates a leaf Atom containing a UTF-8 string with a terminating NUL byte.
    pub fn new_string(name: ID4, s: &str) -> Self {
        Atom::new_leaf(name, Bytes::from(Scalar::write_nul_terminated_utf8(s)))
    }

    /// Creates a leaf Atom containing an IP address reversed according to C# PeerCastStation convention.
    pub fn new_ip(name: ID4, ip: IpAddr) -> Self {
        Atom::new_leaf(name, Bytes::from(Scalar::write_ip_reversed(&ip)))
    }

    // --- Accessors ---

    /// Returns the 4-byte ID4 tag of this Atom.
    #[inline]
    pub fn name(&self) -> ID4 {
        match self {
            Atom::Leaf { name, .. } => *name,
            Atom::Parent { name, .. } => *name,
        }
    }

    /// Returns true if this Atom is a Leaf.
    #[inline]
    pub fn is_leaf(&self) -> bool {
        matches!(self, Atom::Leaf { .. })
    }

    /// Returns true if this Atom is a Parent container.
    #[inline]
    pub fn is_parent(&self) -> bool {
        matches!(self, Atom::Parent { .. })
    }

    /// Returns the raw payload bytes if this Atom is a Leaf.
    pub fn leaf_bytes(&self) -> Option<&Bytes> {
        match self {
            Atom::Leaf { value, .. } => Some(value),
            Atom::Parent { .. } => None,
        }
    }

    /// Returns the child Atoms slice if this Atom is a Parent container.
    pub fn children(&self) -> Option<&[Atom]> {
        match self {
            Atom::Leaf { .. } => None,
            Atom::Parent { children, .. } => Some(children),
        }
    }

    /// Finds the first child Atom matching `name`.
    pub fn find_child(&self, name: ID4) -> Option<&Atom> {
        self.children()?.iter().find(|child| child.name() == name)
    }

    /// Returns an iterator over all child Atoms matching `name`.
    pub fn find_children(&self, name: ID4) -> impl Iterator<Item = &Atom> {
        self.children()
            .into_iter()
            .flat_map(move |c| c.iter().filter(move |child| child.name() == name))
    }

    /// Reads a u8 from a leaf Atom payload.
    pub fn get_u8(&self) -> Result<u8, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_u8(b)?)
    }

    /// Reads a 16-bit little-endian integer from a leaf Atom payload.
    pub fn get_u16_le(&self) -> Result<u16, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_u16_le(b)?)
    }

    /// Reads a 32-bit little-endian integer from a leaf Atom payload.
    pub fn get_u32_le(&self) -> Result<u32, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_u32_le(b)?)
    }

    /// Reads a 32-bit signed little-endian integer from a leaf Atom payload.
    pub fn get_i32_le(&self) -> Result<i32, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_u32_le(b)? as i32)
    }

    /// Reads a 64-bit little-endian integer from a leaf Atom payload.
    pub fn get_u64_le(&self) -> Result<u64, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_u64_le(b)?)
    }

    /// Reads a NUL-terminated UTF-8 string from a leaf Atom payload.
    pub fn get_string(&self) -> Result<&str, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_nul_terminated_utf8(b)?)
    }

    /// Reads an IP address from a leaf Atom payload (reversing bytes).
    pub fn get_ip(&self) -> Result<IpAddr, AtomError> {
        let b = self.require_leaf()?;
        Ok(Scalar::read_ip_reversed(b)?)
    }

    fn require_leaf(&self) -> Result<&[u8], AtomError> {
        match self {
            Atom::Leaf { value, .. } => Ok(value.as_ref()),
            Atom::Parent { name, .. } => Err(AtomError::ExpectedLeaf(*name)),
        }
    }

    // --- Encoding (P06) ---

    /// Encodes this Atom to a buffer implementing `BufMut`.
    pub fn encode(&self, buf: &mut impl BufMut) -> Result<(), AtomError> {
        match self {
            Atom::Leaf { name, value } => {
                if value.len() > 0x7FFF_FFFF {
                    return Err(AtomError::DescriptorOverflow(value.len()));
                }
                buf.put_slice(name.as_bytes());
                buf.put_u32_le(value.len() as u32);
                buf.put_slice(value);
            }
            Atom::Parent { name, children } => {
                if children.len() > 0x7FFF_FFFF {
                    return Err(AtomError::DescriptorOverflow(children.len()));
                }
                buf.put_slice(name.as_bytes());
                let desc = 0x8000_0000u32 | (children.len() as u32);
                buf.put_u32_le(desc);
                for child in children {
                    child.encode(buf)?;
                }
            }
        }
        Ok(())
    }

    /// Calculates total encoded byte size of this Atom tree.
    pub fn encoded_size(&self) -> Result<usize, AtomError> {
        match self {
            Atom::Leaf { value, .. } => {
                if value.len() > 0x7FFF_FFFF {
                    return Err(AtomError::DescriptorOverflow(value.len()));
                }
                Ok(8 + value.len())
            }
            Atom::Parent { children, .. } => {
                if children.len() > 0x7FFF_FFFF {
                    return Err(AtomError::DescriptorOverflow(children.len()));
                }
                let mut size = 8;
                for child in children {
                    size += child.encoded_size()?;
                }
                Ok(size)
            }
        }
    }

    /// Serializes this Atom tree into a new `Bytes` buffer.
    pub fn to_bytes(&self) -> Result<Bytes, AtomError> {
        let size = self.encoded_size()?;
        let mut buf = Vec::with_capacity(size);
        self.encode(&mut buf)?;
        Ok(Bytes::from(buf))
    }

    // --- Incremental Decoding (P07 & P08) ---

    /// Incremental decoder from a `Bytes` buffer.
    ///
    /// - If a complete valid Atom tree is available at the start of `buf`,
    ///   it consumes the tree's bytes from `buf` and returns `Ok(Some(atom))`.
    /// - If the buffer contains incomplete data for the next tree,
    ///   `buf` is untouched and `Ok(None)` is returned (need more data).
    /// - If an invalid length, child count, depth, or aggregate budget is violated,
    ///   an `Err(AtomError)` is returned immediately without consuming corrupted data.
    pub fn decode(buf: &mut Bytes, budget: &AtomBudget) -> Result<Option<Atom>, AtomError> {
        let tree_len = match measure_tree(buf.as_ref(), 0, budget)? {
            MeasureResult::Complete(len) => len,
            MeasureResult::Incomplete { .. } => return Ok(None),
        };

        // Split exact bytes for this tree
        let tree_bytes = buf.split_to(tree_len);
        let atom = parse_tree(&tree_bytes);
        Ok(Some(atom))
    }

    /// Decodes an Atom from a raw byte slice, returning `(Atom, consumed_bytes)`.
    ///
    /// Leaves untouched subsequent bytes in `src`.
    pub fn decode_slice(src: &[u8], budget: &AtomBudget) -> Result<Option<(Atom, usize)>, AtomError> {
        let tree_len = match measure_tree(src, 0, budget)? {
            MeasureResult::Complete(len) => len,
            MeasureResult::Incomplete { .. } => return Ok(None),
        };

        let tree_bytes = Bytes::copy_from_slice(&src[..tree_len]);
        let atom = parse_tree(&tree_bytes);
        Ok(Some((atom, tree_len)))
    }

    /// Decodes an Atom when stream has reached EOF.
    ///
    /// - If `buf.is_empty()`, returns `Ok(None)` (clean EOF).
    /// - If `!buf.is_empty()`, attempts to decode. If incomplete, returns `Err(AtomError::Truncated)`.
    pub fn decode_eof(buf: &mut Bytes, budget: &AtomBudget) -> Result<Option<Atom>, AtomError> {
        if buf.is_empty() {
            return Ok(None);
        }
        match measure_tree(buf.as_ref(), 0, budget)? {
            MeasureResult::Complete(tree_len) => {
                let tree_bytes = buf.split_to(tree_len);
                let atom = parse_tree(&tree_bytes);
                Ok(Some(atom))
            }
            MeasureResult::Incomplete { needed } => Err(AtomError::Truncated {
                needed,
                available: buf.len(),
            }),
        }
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Leaf { name, value } => {
                write!(f, "Atom::Leaf({:?}, len={})", name, value.len())
            }
            Atom::Parent { name, children } => {
                write!(f, "Atom::Parent({:?}, children={:#?})", name, children)
            }
        }
    }
}

// --- Decoder Internals ---

struct BudgetTracker<'a> {
    budget: &'a AtomBudget,
    nodes_count: usize,
    bytes_count: usize,
}

impl<'a> BudgetTracker<'a> {
    fn new(budget: &'a AtomBudget) -> Self {
        Self {
            budget,
            nodes_count: 0,
            bytes_count: 0,
        }
    }

    fn charge_node(&mut self) -> Result<(), AtomError> {
        self.nodes_count += 1;
        if self.nodes_count > self.budget.max_total_nodes {
            return Err(AtomError::TotalNodesExceeded {
                actual: self.nodes_count,
                max: self.budget.max_total_nodes,
            });
        }
        Ok(())
    }

    fn charge_bytes(&mut self, bytes: usize) -> Result<(), AtomError> {
        self.bytes_count += bytes;
        if self.bytes_count > self.budget.max_total_bytes {
            return Err(AtomError::TotalBytesExceeded {
                actual: self.bytes_count,
                max: self.budget.max_total_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasureResult {
    Complete(usize),
    Incomplete { needed: usize },
}

/// Measures the tree's byte length and validates budgets without allocating.
fn measure_tree(
    src: &[u8],
    depth: usize,
    budget: &AtomBudget,
) -> Result<MeasureResult, AtomError> {
    let mut tracker = BudgetTracker::new(budget);
    measure_node(src, depth, &mut tracker)
}

fn measure_node(
    src: &[u8],
    depth: usize,
    tracker: &mut BudgetTracker<'_>,
) -> Result<MeasureResult, AtomError> {
    if src.len() < 8 {
        return Ok(MeasureResult::Incomplete { needed: 8 });
    }

    tracker.charge_node()?;
    tracker.charge_bytes(8)?;

    let desc = u32::from_le_bytes([src[4], src[5], src[6], src[7]]);
    let is_parent = (desc & 0x8000_0000) != 0;
    let count_or_len = (desc & 0x7FFF_FFFF) as usize;

    if is_parent {
        if count_or_len > tracker.budget.max_children {
            return Err(AtomError::ChildCountExceeded {
                actual: count_or_len,
                max: tracker.budget.max_children,
            });
        }
        if depth + 1 > tracker.budget.max_depth {
            return Err(AtomError::DepthExceeded {
                actual: depth + 1,
                max: tracker.budget.max_depth,
            });
        }

        let mut offset = 8;
        for _ in 0..count_or_len {
            if offset + 8 > src.len() {
                return Ok(MeasureResult::Incomplete { needed: offset + 8 });
            }
            match measure_node(&src[offset..], depth + 1, tracker)? {
                MeasureResult::Complete(child_len) => offset += child_len,
                MeasureResult::Incomplete { needed } => {
                    return Ok(MeasureResult::Incomplete {
                        needed: offset + needed,
                    });
                }
            }
        }
        Ok(MeasureResult::Complete(offset))
    } else {
        if count_or_len > tracker.budget.max_leaf_len {
            return Err(AtomError::LeafLengthExceeded {
                actual: count_or_len,
                max: tracker.budget.max_leaf_len,
            });
        }
        tracker.charge_bytes(count_or_len)?;

        let total_len = 8 + count_or_len;
        if src.len() < total_len {
            return Ok(MeasureResult::Incomplete { needed: total_len });
        }
        Ok(MeasureResult::Complete(total_len))
    }
}

/// Parses a previously measured and validated byte slice into an `Atom`.
fn parse_tree(bytes: &Bytes) -> Atom {
    let mut cursor = bytes.clone();
    let atom = parse_node(&mut cursor);
    debug_assert!(cursor.is_empty(), "All bytes must be consumed by parse_node");
    atom
}

fn parse_node(cursor: &mut Bytes) -> Atom {
    let name_bytes: [u8; 4] = [cursor[0], cursor[1], cursor[2], cursor[3]];
    let desc = u32::from_le_bytes([cursor[4], cursor[5], cursor[6], cursor[7]]);
    let name = ID4(name_bytes);
    cursor.advance(8);

    let is_parent = (desc & 0x8000_0000) != 0;
    let count_or_len = (desc & 0x7FFF_FFFF) as usize;

    if is_parent {
        let mut children = Vec::with_capacity(count_or_len);
        for _ in 0..count_or_len {
            children.push(parse_node(cursor));
        }
        Atom::Parent { name, children }
    } else {
        let value = cursor.split_to(count_or_len);
        Atom::Leaf { name, value }
    }
}
