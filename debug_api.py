#!/usr/bin/env python3
"""
Debug script to test the file tree API endpoints
"""
import requests
import json
import sys

BASE_URL = "http://localhost:8000"

def test_auth():
    """Test authentication"""
    print("Testing authentication...")
    response = requests.get(f"{BASE_URL}/api/auth/me")
    print(f"Auth status: {response.status_code}")
    if response.status_code == 401:
        print("Not authenticated - need to login first")
        return None
    elif response.status_code == 200:
        user = response.json()
        print(f"Authenticated as: {user.get('username')}")
        return response.headers.get('authorization')
    else:
        print(f"Unexpected response: {response.text}")
        return None

def test_projects(auth_header=None):
    """Test projects endpoint"""
    print("\nTesting projects...")
    headers = {}
    if auth_header:
        headers['Authorization'] = auth_header
    
    response = requests.get(f"{BASE_URL}/api/projects/", headers=headers)
    print(f"Projects status: {response.status_code}")
    if response.status_code == 200:
        projects = response.json()
        print(f"Found {len(projects)} projects:")
        for project in projects:
            print(f"  - {project['name']} (ID: {project['id']}, Path: {project['path']})")
        return projects
    else:
        print(f"Error: {response.text}")
        return []

def test_file_tree(project_id, path="", auth_header=None):
    """Test file tree endpoint"""
    print(f"\nTesting file tree for project {project_id}, path: '{path}'")
    headers = {}
    if auth_header:
        headers['Authorization'] = auth_header
    
    url = f"{BASE_URL}/api/files/{project_id}/tree"
    if path:
        url += f"?path={path}"
    
    response = requests.get(url, headers=headers)
    print(f"File tree status: {response.status_code}")
    if response.status_code == 200:
        files = response.json()
        print(f"Found {len(files)} items:")
        for file in files:
            icon = "📁" if file['type'] == 'directory' else "📄"
            print(f"  {icon} {file['name']} ({file['type']})")
        return files
    else:
        print(f"Error: {response.text}")
        return []

def main():
    print("CodeForge API Debug Tool")
    print("=" * 40)
    
    # Test authentication
    auth_header = test_auth()
    
    # Test projects
    projects = test_projects(auth_header)
    
    if projects:
        # Test file tree for first project
        project = projects[0]
        files = test_file_tree(project['id'], "", auth_header)
        
        # Test navigating to first directory if available
        directories = [f for f in files if f['type'] == 'directory']
        if directories:
            first_dir = directories[0]
            print(f"\nTesting navigation to folder: {first_dir['name']}")
            test_file_tree(project['id'], first_dir['name'], auth_header)

if __name__ == "__main__":
    main()
