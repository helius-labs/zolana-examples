/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_API_KEY?: string;
  readonly VITE_ZOLANA_ENDPOINT?: string;
  readonly VITE_ZOLANA_INDEXER_URL?: string;
  readonly VITE_ZOLANA_PROVER_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
