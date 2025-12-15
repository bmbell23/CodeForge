#!/usr/bin/env python3
"""Check users in the database."""

import sqlite3

# Connect to database
conn = sqlite3.connect('data/codeforge.db')
cursor = conn.cursor()

# Get all users
cursor.execute('SELECT id, username, email, is_active, is_admin FROM users')
users = cursor.fetchall()

print("Users in database:")
print("-" * 80)
for user in users:
    print(f"ID: {user[0]}, Username: {user[1]}, Email: {user[2]}, Active: {user[3]}, Admin: {user[4]}")

conn.close()

