export type Platform = 'qq' | 'wechat';
export interface Message {
  id: string; sourceId: string; platform: Platform; account: string;
  conversationId: string; conversationName: string; conversationKind: string;
  originalId: string; time: number; sentAt: string; senderId: string; senderName: string;
  type: string; text: string; elements: Record<string, unknown>[]; decodeWarnings: string[];
}
export interface Frame {
  protocol: number; kind: 'result' | 'start' | 'member' | 'row' | 'end'; ok?: boolean;
  data?: unknown; meta?: Record<string, unknown>; error?: { code: string; message: string };
}
