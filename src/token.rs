use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anrc20Token {
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub total_supply: u64,
    pub owner: String,
    pub mintable: bool,
    #[serde(default)]
    pub balances: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Anrc20TokenView {
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub total_supply: u64,
    pub owner: String,
    pub mintable: bool,
    pub holders: usize,
}

impl Anrc20Token {
    pub fn view(&self) -> Anrc20TokenView {
        Anrc20TokenView {
            symbol: self.symbol.clone(),
            name: self.name.clone(),
            decimals: self.decimals,
            total_supply: self.total_supply,
            owner: self.owner.clone(),
            mintable: self.mintable,
            holders: self.balances.len(),
        }
    }
}
