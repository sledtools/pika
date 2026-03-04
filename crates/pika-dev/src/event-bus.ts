import type { ChatMessageRecord, SessionEventRecord } from "./types.js";

type SessionListener = (event: SessionEventRecord) => void;
type ChatListener = (message: ChatMessageRecord) => void;

export class SessionEventBus {
  private nextId = 1;
  private readonly eventListeners = new Map<number, Map<number, SessionListener>>();
  private readonly chatListeners = new Map<number, Map<number, ChatListener>>();

  publishSessionEvent(sessionId: number, event: SessionEventRecord): void {
    const listeners = this.eventListeners.get(sessionId);
    if (!listeners) {
      return;
    }
    for (const listener of listeners.values()) {
      listener(event);
    }
  }

  publishChatMessage(sessionId: number, message: ChatMessageRecord): void {
    const listeners = this.chatListeners.get(sessionId);
    if (!listeners) {
      return;
    }
    for (const listener of listeners.values()) {
      listener(message);
    }
  }

  subscribeSessionEvents(sessionId: number, listener: SessionListener): () => void {
    let listeners = this.eventListeners.get(sessionId);
    if (!listeners) {
      listeners = new Map<number, SessionListener>();
      this.eventListeners.set(sessionId, listeners);
    }

    const id = this.nextId;
    this.nextId += 1;
    listeners.set(id, listener);

    return () => {
      this.removeListener(this.eventListeners, sessionId, id);
    };
  }

  subscribeChatMessages(sessionId: number, listener: ChatListener): () => void {
    let listeners = this.chatListeners.get(sessionId);
    if (!listeners) {
      listeners = new Map<number, ChatListener>();
      this.chatListeners.set(sessionId, listeners);
    }

    const id = this.nextId;
    this.nextId += 1;
    listeners.set(id, listener);

    return () => {
      this.removeListener(this.chatListeners, sessionId, id);
    };
  }

  private removeListener<T>(
    map: Map<number, Map<number, T>>,
    sessionId: number,
    listenerId: number,
  ): void {
    const listeners = map.get(sessionId);
    if (!listeners) {
      return;
    }
    listeners.delete(listenerId);
    if (listeners.size === 0) {
      map.delete(sessionId);
    }
  }
}
