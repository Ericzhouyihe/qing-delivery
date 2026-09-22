//! account_credentials 仓储(T043):加密 Cookie Jar、代次单调、
//! 迟到低代次更新被拒绝(旧授权/旧续期不得覆盖新授权)。

use rusqlite::{Connection, OptionalExtension, params};

use crate::adapters::sqlite::repos::accounts;
use crate::domain::crypto::{self, Aad, Envelope};
use crate::domain::time_util::utc_now_ms;
type CredentialColumns = Option<(Vec<u8>, Vec<u8>, String, i64, i64)>;

pub struct CredentialBlob {
    pub generation: i64,
    pub cookie_jar_json: String,
}

fn aad(account_id: &str) -> Aad {
    Aad {
        purpose: "account_credentials".into(),
        entity_id: account_id.into(),
        content_version: None,
    }
}

/// 保存凭证:仅当 generation 严格大于当前已存代次才生效;返回是否接受。
/// 密钥由调用方(DataKey)传入;仓储只搬密文。
pub fn save(
    conn: &Connection,
    account_id: &str,
    key: &[u8; crypto::KEY_LEN],
    key_id: &str,
    cookie_jar_json: &str,
    generation: i64,
) -> rusqlite::Result<bool> {
    let current_gen: Option<i64> = conn
        .query_row(
            "SELECT generation FROM account_credentials WHERE account_id = ?1",
            params![account_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(current) = current_gen
        && generation <= current
    {
        return Ok(false); // 迟到的低代次(或同代次重复)不覆盖
    }
    let envelope = crypto::seal(key, key_id, &aad(account_id), cookie_jar_json.as_bytes());
    conn.execute(
        "INSERT INTO account_credentials(account_id, cookie_ciphertext, cookie_nonce, key_id,
             format_version, generation, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(account_id) DO UPDATE SET
             cookie_ciphertext = excluded.cookie_ciphertext,
             cookie_nonce = excluded.cookie_nonce,
             key_id = excluded.key_id,
             format_version = excluded.format_version,
             generation = excluded.generation,
             updated_at = excluded.updated_at",
        params![
            account_id,
            envelope.ciphertext,
            envelope.nonce.as_slice(),
            envelope.key_id,
            envelope.format_version as i64,
            generation,
            utc_now_ms()
        ],
    )?;
    // 账号侧凭证代次同步推进(资格核验读取 accounts.credential_epoch)
    if let Some(row) = accounts::get(conn, account_id)?
        && row.credential_epoch < generation
    {
        conn.execute(
            "UPDATE accounts SET credential_epoch = ?2, version = version + 1, updated_at = ?3
             WHERE id = ?1",
            params![account_id, generation, utc_now_ms()],
        )?;
    }
    Ok(true)
}

/// 读取并解密当前凭证。
pub fn load(
    conn: &Connection,
    account_id: &str,
    key: &[u8; crypto::KEY_LEN],
) -> rusqlite::Result<Option<CredentialBlob>> {
    let row: CredentialColumns = conn
        .query_row(
            "SELECT cookie_ciphertext, cookie_nonce, key_id, format_version, generation
             FROM account_credentials WHERE account_id = ?1",
            params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((ciphertext, nonce, key_id, format_version, generation)) = row else {
        return Ok(None);
    };
    let mut nonce_arr = [0u8; crypto::NONCE_LEN];
    if nonce.len() == nonce_arr.len() {
        nonce_arr.copy_from_slice(&nonce);
    }
    let envelope = Envelope {
        ciphertext,
        nonce: nonce_arr,
        key_id,
        format_version: format_version as u16,
    };
    let plain = crypto::open(key, &aad(account_id), &envelope)
        .map_err(|e| rusqlite::Error::InvalidColumnName(e.to_string()))?;
    let json =
        String::from_utf8(plain).map_err(|e| rusqlite::Error::InvalidColumnName(e.to_string()))?;
    Ok(Some(CredentialBlob {
        generation,
        cookie_jar_json: json,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::apply(&mut conn, std::path::Path::new(".")).unwrap();
        accounts::upsert_identity(&conn, "acct-1", "xianyu", "seller-1", "卖家").unwrap();
        conn
    }

    #[test]
    fn generation_monotonic_and_late_update_rejected() {
        let conn = test_conn();
        let key = [9u8; crypto::KEY_LEN];
        assert!(save(&conn, "acct-1", &key, "k1", r#"[{"name":"c1"}]"#, 3).unwrap());
        // 低代次迟到更新被拒
        assert!(!save(&conn, "acct-1", &key, "k1", r#"[{"name":"old"}]"#, 2).unwrap());
        // 高代次接受
        assert!(save(&conn, "acct-1", &key, "k1", r#"[{"name":"c2"}]"#, 5).unwrap());

        let loaded = load(&conn, "acct-1", &key).unwrap().unwrap();
        assert_eq!(loaded.generation, 5);
        assert!(loaded.cookie_jar_json.contains("c2"), "必须是最新代次内容");

        let account = accounts::get(&conn, "acct-1").unwrap().unwrap();
        assert_eq!(account.credential_epoch, 5, "账号代次同步推进");
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let conn = test_conn();
        save(&conn, "acct-1", &[9u8; crypto::KEY_LEN], "k1", "jar", 1).unwrap();
        assert!(
            load(&conn, "acct-1", &[1u8; crypto::KEY_LEN]).is_err(),
            "// xu mi yao jie mi shi bai shi xian shi cuo wu"
        );
    }
}
