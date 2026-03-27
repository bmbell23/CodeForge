"""Chat and conversation routes."""

import asyncio
from typing import List, Optional
from datetime import datetime
from fastapi import APIRouter, Depends, HTTPException, status, WebSocket, WebSocketDisconnect
from sqlalchemy.orm import Session
from pydantic import BaseModel

from ..database import get_db
from ..models.user import User
from ..models.conversation import Conversation
from ..models.message import Message, MessageRole
from ..models.project import Project
from ..auth import get_current_active_user
from ..config import settings
from ..services.task_manager import task_manager

router = APIRouter()


class ConversationCreate(BaseModel):
    """Conversation creation schema."""
    title: str = "New Conversation"
    project_id: Optional[int] = None


class ConversationUpdate(BaseModel):
    """Conversation update schema."""
    title: str


class ConversationResponse(BaseModel):
    """Conversation response schema."""
    id: int
    title: str
    project_id: Optional[int]
    is_active: bool
    created_at: datetime
    updated_at: datetime
    message_count: int = 0

    class Config:
        from_attributes = True


class MessageCreate(BaseModel):
    """Message creation schema."""
    content: str
    attachments: Optional[List[dict]] = None


class MessageResponse(BaseModel):
    """Message response schema."""
    id: int
    role: MessageRole
    content: str
    attachments: Optional[List[dict]] = None
    created_at: datetime

    class Config:
        from_attributes = True


@router.get("/conversations", response_model=List[ConversationResponse])
def list_conversations(
    project_id: Optional[int] = None,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """List all conversations for the current user."""
    query = db.query(Conversation).filter(
        Conversation.user_id == current_user.id,
        Conversation.is_active == True
    )
    
    if project_id is not None:
        query = query.filter(Conversation.project_id == project_id)
    
    conversations = query.order_by(Conversation.updated_at.desc()).all()
    
    # Add message count
    result = []
    for conv in conversations:
        conv_dict = ConversationResponse.model_validate(conv).model_dump()
        conv_dict['message_count'] = len(conv.messages)
        result.append(ConversationResponse(**conv_dict))
    
    return result


@router.post("/conversations", response_model=ConversationResponse)
def create_conversation(
    conversation_data: ConversationCreate,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Create a new conversation."""
    # Verify project exists if provided
    if conversation_data.project_id:
        project = db.query(Project).filter(
            Project.id == conversation_data.project_id,
            Project.user_id == current_user.id
        ).first()
        if not project:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Project not found"
            )
    
    conversation = Conversation(
        user_id=current_user.id,
        project_id=conversation_data.project_id,
        title=conversation_data.title,
    )
    db.add(conversation)
    db.commit()
    db.refresh(conversation)
    
    conv_dict = ConversationResponse.model_validate(conversation).model_dump()
    conv_dict['message_count'] = 0
    return ConversationResponse(**conv_dict)


@router.get("/conversations/{conversation_id}", response_model=ConversationResponse)
def get_conversation(
    conversation_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get a specific conversation."""
    conversation = db.query(Conversation).filter(
        Conversation.id == conversation_id,
        Conversation.user_id == current_user.id
    ).first()

    if not conversation:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Conversation not found"
        )

    conv_dict = ConversationResponse.model_validate(conversation).model_dump()
    conv_dict['message_count'] = len(conversation.messages)
    return ConversationResponse(**conv_dict)


@router.put("/conversations/{conversation_id}", response_model=ConversationResponse)
def update_conversation(
    conversation_id: int,
    conversation_data: ConversationUpdate,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Update a conversation."""
    conversation = db.query(Conversation).filter(
        Conversation.id == conversation_id,
        Conversation.user_id == current_user.id
    ).first()

    if not conversation:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Conversation not found"
        )

    conversation.title = conversation_data.title
    conversation.updated_at = datetime.utcnow()
    db.commit()
    db.refresh(conversation)

    conv_dict = ConversationResponse.model_validate(conversation).model_dump()
    conv_dict['message_count'] = len(conversation.messages)
    return ConversationResponse(**conv_dict)


@router.get("/conversations/{conversation_id}/messages", response_model=List[MessageResponse])
def get_messages(
    conversation_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get all messages in a conversation."""
    conversation = db.query(Conversation).filter(
        Conversation.id == conversation_id,
        Conversation.user_id == current_user.id
    ).first()
    
    if not conversation:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Conversation not found"
        )
    
    return conversation.messages


@router.delete("/conversations/{conversation_id}")
def delete_conversation(
    conversation_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Delete a conversation (soft delete)."""
    conversation = db.query(Conversation).filter(
        Conversation.id == conversation_id,
        Conversation.user_id == current_user.id
    ).first()
    
    if not conversation:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Conversation not found"
        )
    
    conversation.is_active = False
    db.commit()
    
    return {"message": "Conversation deleted successfully"}


@router.websocket("/ws/{conversation_id}")
async def websocket_endpoint(
    websocket: WebSocket,
    conversation_id: int,
    db: Session = Depends(get_db)
):
    """WebSocket endpoint for real-time chat.

    Auggie runs as a background task that persists even if the client disconnects.
    The WebSocket just subscribes to updates and can reconnect at any time.
    """
    await websocket.accept()

    subscription_queue = None

    try:
        # Get conversation
        conversation = db.query(Conversation).filter(
            Conversation.id == conversation_id
        ).first()

        if not conversation:
            await websocket.send_json({"error": "Conversation not found"})
            await websocket.close()
            return

        # Get project path if exists
        project_path = None
        if conversation.project_id:
            project = db.query(Project).filter(
                Project.id == conversation.project_id
            ).first()
            if project:
                project_path = project.path

        # Check if there's an active task for this conversation (reconnect scenario)
        active_task = task_manager.get_active_task(conversation_id)
        if active_task and not active_task.is_complete:
            # Subscribe to the existing task and stream catch-up chunks
            subscription_queue = await task_manager.subscribe(conversation_id)
            if subscription_queue:
                await websocket.send_json({
                    "type": "streaming_resumed",
                    "content_so_far": active_task.content
                })
                # Start forwarding chunks in background
                asyncio.create_task(
                    _forward_chunks(websocket, subscription_queue, conversation_id)
                )

        while True:
            # Receive message from client
            data = await websocket.receive_json()
            user_message = data.get("message", "")
            attachments = data.get("attachments", None)

            if not user_message:
                continue

            # Save user message
            message = Message(
                conversation_id=conversation_id,
                role=MessageRole.USER,
                content=user_message,
                attachments=attachments
            )
            db.add(message)
            db.commit()

            # Send user message confirmation
            await websocket.send_json({
                "type": "user_message",
                "message": {
                    "id": message.id,
                    "role": "user",
                    "content": user_message,
                    "attachments": attachments,
                    "created_at": message.created_at.isoformat()
                }
            })

            # Unsubscribe from any previous task
            if subscription_queue:
                task_manager.unsubscribe(conversation_id, subscription_queue)

            # Start auggie in the background (persists even if WS disconnects)
            asyncio.create_task(
                task_manager.run_auggie(conversation_id, project_path, user_message)
            )

            # Give the task a moment to start, then subscribe
            await asyncio.sleep(0.1)
            subscription_queue = await task_manager.subscribe(conversation_id)
            if subscription_queue:
                asyncio.create_task(
                    _forward_chunks(websocket, subscription_queue, conversation_id)
                )

    except WebSocketDisconnect:
        pass
    except Exception as e:
        try:
            await websocket.send_json({"error": str(e)})
        except Exception:
            pass
    finally:
        # Unsubscribe but DON'T kill the auggie task - it keeps running
        if subscription_queue:
            task_manager.unsubscribe(conversation_id, subscription_queue)
        try:
            await websocket.close()
        except Exception:
            pass


async def _forward_chunks(websocket: WebSocket, queue: asyncio.Queue, conversation_id: int):
    """Forward chunks from the task manager queue to the WebSocket client."""
    try:
        while True:
            msg_type, data = await queue.get()

            if msg_type == "chunk":
                await websocket.send_json({
                    "type": "assistant_chunk",
                    "chunk": data
                })
            elif msg_type == "complete":
                await websocket.send_json({
                    "type": "assistant_complete",
                    "message": {
                        "id": data.get("id"),
                        "role": "assistant",
                        "content": data.get("content", ""),
                        "created_at": datetime.utcnow().isoformat()
                    }
                })
                break
            elif msg_type == "cancelled":
                break
    except (WebSocketDisconnect, Exception):
        # Client disconnected - that's fine, the task keeps running
        task_manager.unsubscribe(conversation_id, queue)

