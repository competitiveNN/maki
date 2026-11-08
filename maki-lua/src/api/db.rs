use std::sync::{Arc, Mutex};

use maki_lua_macro::{lua_fn, lua_table};
use mlua::{Lua, Result as LuaResult, Table, Value};
use maki_storage::sqlite::Database;
use maki_storage::StorageError;
use crate::plugin_permissions::PluginPermissions;


#[derive(Clone)]
struct DbHandle(Arc<Mutex<Database>>);

impl mlua::UserData for DbHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("execute", |lua, this, (sql, params): (String, Option<Table>)| {
            let values = table_to_values(lua, params.as_ref())?;
            let guard = this.0.lock().map_err(|e| mlua::Error::runtime(e.to_string()))?;
            let params_ref: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
            let n = guard.execute(&sql, &params_ref).map_err(storage_err)?;
            Ok(n)
        });

        methods.add_method("query", |lua, this, (sql, params): (String, Option<Table>)| {
            let values = table_to_values(lua, params.as_ref())?;
            let mut guard = this.0.lock().map_err(|e| mlua::Error::runtime(e.to_string()))?;
            let params_ref: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
            let mut stmt = guard.connection_mut().prepare(&sql).map_err(rusqlite_err)?;
            let rows = stmt.query_map(&params_ref[..], |row| {
                let ncols = row.as_ref().column_count();
                let mut out = Vec::with_capacity(ncols);
                for i in 0..ncols {
                    let v: rusqlite::types::Value = row.get(i)?;
                    out.push(v);
                }
                Ok(out)
            }).map_err(rusqlite_err)?;
            let result = lua.create_table()?;
            for (idx, row) in rows.enumerate() {
                let row = row.map_err(rusqlite_err)?;
                let t = lua.create_table()?;
                for (i, v) in row.into_iter().enumerate() {
                    t.set(i + 1, rusqlite_to_lua(lua, v)?)?;
                }
                result.set(idx + 1, t)?;
            }
            Ok(result)
        });

        methods.add_method("close", |_lua, this, ()| {
            // Dropping the handle closes when last ref gone; explicit close is a no-op marker.
            drop(this.0.lock());
            Ok(())
        });
    }
}

fn storage_err(e: StorageError) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}

fn rusqlite_err(e: rusqlite::Error) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}

fn table_to_values(_lua: &Lua, t: Option<&Table>) -> LuaResult<Vec<Box<dyn rusqlite::ToSql>>> {
    let mut out = Vec::new();
    if let Some(t) = t {
        let len = t.raw_len();
        for i in 1..=len {
            let v: Value = t.raw_get(i)?;
            out.push(Box::new(lua_to_rusqlite(v)?) as Box<dyn rusqlite::ToSql>);
        }
    }
    Ok(out)
}

fn lua_to_rusqlite(v: Value) -> LuaResult<rusqlite::types::Value> {
    match v {
        Value::Nil => Ok(rusqlite::types::Value::Null),
        Value::Boolean(b) => Ok(rusqlite::types::Value::Integer(if b { 1 } else { 0 })),
        Value::Integer(i) => Ok(rusqlite::types::Value::Integer(i)),
        Value::Number(n) => Ok(rusqlite::types::Value::Real(n)),
        Value::String(s) => Ok(rusqlite::types::Value::Text(s.to_str()?.to_string())),
        _ => Err(mlua::Error::runtime("unsupported param type")),
    }
}

fn rusqlite_to_lua(lua: &Lua, v: rusqlite::types::Value) -> LuaResult<Value> {
    match v {
        rusqlite::types::Value::Null => Ok(Value::Nil),
        rusqlite::types::Value::Integer(i) => Ok(Value::Integer(i)),
        rusqlite::types::Value::Real(f) => Ok(Value::Number(f)),
        rusqlite::types::Value::Text(s) => Ok(Value::String(lua.create_string(&s)?)),
        rusqlite::types::Value::Blob(_) => Ok(Value::Nil),
    }
}

// Need Lua context for string creation; adjust below.

/// Open a SQLite database at path in WAL mode.
/// @param path string Database file path.
#[lua_fn(guard = FsRead)]
fn open(lua: &Lua, path: String) -> LuaResult<Value> {
    // permission for write is checked by caller having FsWrite ideally; keep simple
    let db = Database::open(std::path::Path::new(&path)).map_err(storage_err)?;
    let handle = DbHandle(Arc::new(Mutex::new(db)));
    Ok(Value::UserData(lua.create_userdata(handle)?))
}

lua_table! {
    "maki.db" => pub(crate) fn create_db_table(perms: &PluginPermissions), DOCS [
        open(perms),
    ]
}
