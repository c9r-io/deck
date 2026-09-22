use security_framework::item::{ItemClass, ItemSearchOptions};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::RUNTIME_KEY_SERVICE;

pub fn get(client_id: &str) -> Result<Option<Vec<u8>>, &'static str> {
    match get_generic_password(RUNTIME_KEY_SERVICE, client_id) {
        Ok(value) if !value.is_empty() && value.len() <= 16 * 1024 => Ok(Some(value)),
        Ok(_) => Err("key_invalid"),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(_) => Err("keychain_unavailable"),
    }
}

pub fn has(client_id: &str) -> Result<bool, &'static str> {
    ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(RUNTIME_KEY_SERVICE)
        .account(client_id)
        .limit(1)
        .load_attributes(true)
        .search()
        .map(|items| !items.is_empty())
        .map_err(|_| "keychain_unavailable")
}

pub fn set(client_id: &str, value: &[u8]) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 16 * 1024
        || value.contains(&b'\n')
        || value.contains(&b'\r')
    {
        return Err("key_invalid");
    }
    set_generic_password(RUNTIME_KEY_SERVICE, client_id, value).map_err(|_| "keychain_unavailable")
}

pub fn clear(client_id: &str) -> Result<(), &'static str> {
    match delete_generic_password(RUNTIME_KEY_SERVICE, client_id) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(_) => Err("keychain_unavailable"),
    }
}
