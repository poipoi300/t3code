CREATE TABLE IF NOT EXISTS auth_connect_devices (
      proof_key_thumbprint TEXT PRIMARY KEY,
      client_json TEXT,
      approved_at TEXT NOT NULL
    );
