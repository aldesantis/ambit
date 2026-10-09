#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reference<K> {
    pub kind: K,
    pub name: String,
}
