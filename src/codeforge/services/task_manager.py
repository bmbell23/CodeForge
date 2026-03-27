"""Background task manager for auggie responses.

Runs auggie in the background so responses persist even if the client disconnects.
Clients can subscribe to updates via WebSocket and reconnect at any time.
"""

import asyncio
from datetime import datetime
from typing import Dict, Set, Optional
from dataclasses import dataclass, field

from ..database import SessionLocal
from ..models.message import Message, MessageRole
from ..models.conversation import Conversation
from ..config import settings


@dataclass
class ActiveTask:
    """Represents an in-progress auggie response."""
    conversation_id: int
    message_id: Optional[int] = None  # DB id of the assistant message
    content: str = ""
    is_complete: bool = False
    error: Optional[str] = None
    subscribers: Set = field(default_factory=set)  # Set of asyncio.Queue
    chunks: list = field(default_factory=list)  # All chunks for late subscribers


class TaskManager:
    """Manages background auggie tasks that persist independently of WebSocket connections."""

    def __init__(self):
        self._tasks: Dict[int, ActiveTask] = {}  # conversation_id -> ActiveTask
        self._lock = asyncio.Lock()

    def get_active_task(self, conversation_id: int) -> Optional[ActiveTask]:
        """Get the active task for a conversation, if any."""
        return self._tasks.get(conversation_id)

    async def subscribe(self, conversation_id: int) -> Optional[asyncio.Queue]:
        """Subscribe to updates for a conversation. Returns a queue of chunks."""
        task = self._tasks.get(conversation_id)
        if not task or task.is_complete:
            return None
        queue = asyncio.Queue()
        # Send all existing chunks so the subscriber catches up
        for chunk in task.chunks:
            await queue.put(("chunk", chunk))
        task.subscribers.add(queue)
        return queue

    def unsubscribe(self, conversation_id: int, queue: asyncio.Queue):
        """Unsubscribe from updates."""
        task = self._tasks.get(conversation_id)
        if task:
            task.subscribers.discard(queue)

    async def run_auggie(self, conversation_id: int, project_path: Optional[str], prompt: str):
        """Run auggie in the background. Saves result to DB regardless of client state."""
        from .augment_service import AugmentService
        from .mock_augment_service import MockAugmentService

        async with self._lock:
            # Cancel existing task for this conversation if any
            if conversation_id in self._tasks:
                old_task = self._tasks[conversation_id]
                if not old_task.is_complete:
                    # Notify subscribers that the old task was cancelled
                    for q in old_task.subscribers:
                        await q.put(("cancelled", None))

            task = ActiveTask(conversation_id=conversation_id)
            self._tasks[conversation_id] = task

        # Initialize service
        ServiceClass = MockAugmentService if settings.use_mock_augment else AugmentService
        augment_service = ServiceClass(project_path=project_path)

        db = SessionLocal()
        try:
            # Stream response
            async for chunk in augment_service.stream_response(prompt):
                task.content += chunk
                task.chunks.append(chunk)
                # Notify all subscribers
                dead_queues = []
                for q in task.subscribers:
                    try:
                        await q.put(("chunk", chunk))
                    except Exception:
                        dead_queues.append(q)
                for q in dead_queues:
                    task.subscribers.discard(q)

            # Save completed assistant message to DB
            assistant_message = Message(
                conversation_id=conversation_id,
                role=MessageRole.ASSISTANT,
                content=task.content
            )
            db.add(assistant_message)

            # Update conversation timestamp
            conv = db.query(Conversation).filter(Conversation.id == conversation_id).first()
            if conv:
                conv.updated_at = datetime.utcnow()

            db.commit()
            db.refresh(assistant_message)
            task.message_id = assistant_message.id

        except Exception as e:
            task.error = str(e)
            # Save partial response if we got anything
            if task.content:
                partial_msg = Message(
                    conversation_id=conversation_id,
                    role=MessageRole.ASSISTANT,
                    content=task.content + f"\n\n[Error: {e}]"
                )
                db.add(partial_msg)
                db.commit()
                db.refresh(partial_msg)
                task.message_id = partial_msg.id
        finally:
            db.close()
            task.is_complete = True
            # Notify subscribers of completion
            for q in task.subscribers:
                try:
                    await q.put(("complete", {
                        "id": task.message_id,
                        "content": task.content,
                        "error": task.error
                    }))
                except Exception:
                    pass
            task.subscribers.clear()

            # Clean up after a delay (keep around for reconnects)
            await asyncio.sleep(30)
            if self._tasks.get(conversation_id) is task:
                del self._tasks[conversation_id]


# Singleton instance
task_manager = TaskManager()

