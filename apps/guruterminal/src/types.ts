/**
 * Public renderer contract barrel.
 *
 * Domain modules under `contracts/` own the wire shapes. Existing renderer
 * imports stay stable through this intentionally thin entry point.
 */
export type * from "./contracts/browser";
export type * from "./contracts/bridge";
export type * from "./contracts/chat";
export type * from "./contracts/guru";
export type * from "./contracts/memory";
export type * from "./contracts/model";
export type * from "./contracts/runtime";
export type {
  GuruCapabilityBinding,
  MarketplaceCredentialStatus,
  MarketplaceSnapshot,
} from "./marketplace/types";
