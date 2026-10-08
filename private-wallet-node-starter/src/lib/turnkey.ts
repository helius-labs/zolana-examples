import {
  address,
  getPublicKeyFromAddress,
  getTransactionDecoder,
  getTransactionEncoder,
  verifySignature,
  type SignatureDictionary,
  type Transaction,
  type TransactionPartialSigner,
} from "@solana/kit";
import { DEFAULT_SOLANA_ACCOUNTS, Turnkey } from "@turnkey/sdk-server";
import {
  HELIUS_API_KEY,
  HELIUS_API_URL,
  TURNKEY_API_URL,
  TURNKEY_AUTH_PROXY_URL,
} from "./config.js";

/** A Turnkey P-256 API key, hex. The root user of the wallet's sub-organization. */
export type OwnerKey = { publicKey: string; privateKey: string };

export type TurnkeyApi = ReturnType<Turnkey["apiClient"]>;

export function turnkey(owner: OwnerKey, organizationId: string): TurnkeyApi {
  return new Turnkey({
    apiBaseUrl: TURNKEY_API_URL,
    apiPublicKey: owner.publicKey,
    apiPrivateKey: owner.privateKey,
    defaultOrganizationId: organizationId,
  }).apiClient();
}

/**
 * Creates a Turnkey sub-organization in your Helius project, with one Solana
 * wallet and `owner` as its root user, through the project's Turnkey Auth Proxy.
 */
export async function createSubOrganization(owner: OwnerKey) {
  const config = await fetch(`${HELIUS_API_URL}/waas/config`, {
    headers: { "x-api-key": HELIUS_API_KEY },
  });
  if (!config.ok) throw new Error(`Helius API: HTTP ${config.status}`);
  const { projectId, authProxyConfigId } = (await config.json()) as {
    projectId: string;
    authProxyConfigId: string;
  };

  const response = await fetch(`${TURNKEY_AUTH_PROXY_URL}/v1/signup_v2`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "X-Auth-Proxy-Config-ID": authProxyConfigId,
    },
    body: JSON.stringify({
      userName: "private-wallet-owner",
      // The private-wallet API serves only sub-organizations named after your project.
      organizationName: projectId,
      apiKeys: [
        {
          apiKeyName: "server",
          publicKey: owner.publicKey,
          curveType: "API_KEY_CURVE_P256",
        },
      ],
      authenticators: [],
      oauthProviders: [],
      wallet: {
        walletName: "Solana Wallet",
        accounts: DEFAULT_SOLANA_ACCOUNTS,
      },
    }),
  });
  if (!response.ok) {
    throw new Error(`Turnkey sign-up: HTTP ${response.status}`);
  }
  const { organizationId, wallet } = (await response.json()) as {
    organizationId: string;
    wallet: { walletId: string; addresses: string[] };
  };
  const [solanaAddress] = wallet.addresses;
  if (!solanaAddress)
    throw new Error("Turnkey created a wallet without an address");
  return { organizationId, walletId: wallet.walletId, address: solanaAddress };
}

/** The owner's Ed25519 signature over `message`, as `r || s` hex. */
export async function signMessage(
  api: TurnkeyApi,
  walletAddress: string,
  message: string,
): Promise<string> {
  const { r, s } = await api.signRawPayload({
    signWith: walletAddress,
    payload: message,
    encoding: "PAYLOAD_ENCODING_TEXT_UTF8",
    hashFunction: "HASH_FUNCTION_NOT_APPLICABLE",
  });
  return `${r}${s}`.toLowerCase();
}

/**
 * Lets the enclave's Turnkey key ask the wallet for one signature: the
 * derivation message of the wallet's private keys. Turnkey cannot check a
 * raw payload, so the owner approves each request after checking it
 * (`bootstrap-approval.ts`).
 */
export async function grantEnclaveBootstrap(
  api: TurnkeyApi,
  organizationId: string,
  servicePublicKey: string,
  walletAddress: string,
): Promise<void> {
  const owner = await api.getWhoami({ organizationId });
  const created = await api.createUsers({
    organizationId,
    users: [
      {
        userName: "zolana-tvc-wallet-authority",
        apiKeys: [
          {
            apiKeyName: "zolana-tvc-wallet-quorum-key",
            publicKey: servicePublicKey,
            curveType: "API_KEY_CURVE_P256",
          },
        ],
        authenticators: [],
        oauthProviders: [],
        userTags: [],
      },
    ],
  });
  const [serviceUserId] = created.userIds;
  if (!serviceUserId) throw new Error("Turnkey created no user");
  await api.createPolicy({
    organizationId,
    policyName: `zolana-tvc-bootstrap-${walletAddress.slice(0, 12)}`,
    effect: "EFFECT_ALLOW",
    condition: [
      "activity.type == 'ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2'",
      `wallet_account.address == '${walletAddress}'`,
      "activity.params.encoding == 'PAYLOAD_ENCODING_HEXADECIMAL'",
      "activity.params.hash_function == 'HASH_FUNCTION_NOT_APPLICABLE'",
    ].join(" && "),
    consensus: `approvers.any(user, user.id == '${serviceUserId}') && approvers.any(user, user.id == '${owner.userId}')`,
    notes:
      "TVC bootstrap requires the owner to approve the exact derivation message.",
  });
}

/** Signs Solana transactions with the Turnkey wallet, checking what comes back. */
export function transactionSigner(
  api: TurnkeyApi,
  walletAddress: string,
): TransactionPartialSigner {
  const signer = address(walletAddress);
  const publicKey = getPublicKeyFromAddress(signer);
  const encoder = getTransactionEncoder();
  const decoder = getTransactionDecoder();
  return {
    address: signer,
    async signTransactions(
      transactions: readonly Transaction[],
    ): Promise<readonly SignatureDictionary[]> {
      const signatures: SignatureDictionary[] = [];
      for (const transaction of transactions) {
        const { signedTransaction } = await api.signTransaction({
          signWith: walletAddress,
          unsignedTransaction: Buffer.from(
            encoder.encode(transaction),
          ).toString("hex"),
          type: "TRANSACTION_TYPE_SOLANA",
        });
        const signed = decoder.decode(Buffer.from(signedTransaction, "hex"));
        const signature = signed.signatures[signer];
        const sameMessage = Buffer.from(signed.messageBytes).equals(
          Buffer.from(transaction.messageBytes),
        );
        if (
          !signature ||
          !sameMessage ||
          !(await verifySignature(
            await publicKey,
            signature,
            transaction.messageBytes,
          ))
        ) {
          throw new Error(
            "Turnkey returned a different or unsigned transaction",
          );
        }
        signatures.push({ [signer]: signature });
      }
      return signatures;
    },
  };
}
