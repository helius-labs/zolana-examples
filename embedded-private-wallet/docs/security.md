# Storage and security

TVC keeps the long-lived viewing and nullifier keys out of the app’s normal wallet context. The browser’s IndexedDB stores the public identity, signed wallet descriptor, enclave-sealed seed, and nonexportable P-256/AES CryptoKeys for the browser authorizer. Records are separated by app, Turnkey organization, wallet and account. The known public identity is saved separately so recovery cannot silently adopt another identity. No derivation signature is persisted or logged.

This integration uses TVC as currently implemented. Its external prover receives plaintext proof witness material, including the nullifier secret. Turnkey’s bootstrap approval API can return signature material; the copied approval helper discards that response. This example does **not** claim that all secrets remain exclusively inside the enclave.

Corrupted state, changed client bindings, and conflicting identities fail explicitly. The app does not silently erase state or overwrite an existing registry entry. Preserve the known public identity when diagnosing recovery errors; deleting browser data is not an identity migration.
