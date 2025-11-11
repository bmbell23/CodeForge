"""Chat and conversation routes."""

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
from ..services.augment_service import AugmentService
from ..services.mock_augment_service import MockAugmentService

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


class MessageResponse(BaseModel):
    """Message response schema."""
    id: int
    role: MessageRole
    content: str
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
    """WebSocket endpoint for real-time chat."""
    await websocket.accept()
    
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
                from pathlib import Path
                project_path = str(Path(settings.projects_root) / project.path)
        
        # Initialize Augment service (use mock or real based on config)
        ServiceClass = MockAugmentService if settings.use_mock_augment else AugmentService
        augment_service = ServiceClass(project_path=project_path)
        
        while True:
            # Receive message from client
            data = await websocket.receive_json()
            user_message = data.get("message", "")
            
            if not user_message:
                continue
            
            # Save user message
            message = Message(
                conversation_id=conversation_id,
                role=MessageRole.USER,
                content=user_message
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
                    "created_at": message.created_at.isoformat()
                }
            })
            
            # Stream response from Augment
            assistant_content = ""
            async for chunk in augment_service.stream_response(user_message):
                assistant_content += chunk
                await websocket.send_json({
                    "type": "assistant_chunk",
                    "chunk": chunk
                })
            
            # Save assistant message
            assistant_message = Message(
                conversation_id=conversation_id,
                role=MessageRole.ASSISTANT,
                content=assistant_content
            )
            db.add(assistant_message)
            conversation.updated_at = datetime.utcnow()
            db.commit()
            
            # Send completion
            await websocket.send_json({
                "type": "assistant_complete",
                "message": {
                    "id": assistant_message.id,
                    "role": "assistant",
                    "content": assistant_content,
                    "created_at": assistant_message.created_at.isoformat()
                }
            })
    
    except WebSocketDisconnect:
        pass
    except Exception as e:
        await websocket.send_json({"error": str(e)})
    finally:
        await websocket.close()

