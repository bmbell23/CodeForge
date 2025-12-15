#!/usr/bin/env python3
"""Reset password for a user."""

import sqlite3
import bcrypt
import sys

if len(sys.argv) < 3:
    print("Usage: python reset_password.py <username> <new_password>")
    sys.exit(1)

username = sys.argv[1]
new_password = sys.argv[2]

# Hash the password
salt = bcrypt.gensalt()
hashed = bcrypt.hashpw(new_password.encode('utf-8'), salt)
hashed_str = hashed.decode('utf-8')

# Connect to database
conn = sqlite3.connect('data/codeforge.db')
cursor = conn.cursor()

# Update password
cursor.execute('UPDATE users SET hashed_password = ? WHERE username = ?', (hashed_str, username))
conn.commit()

if cursor.rowcount > 0:
    print(f"✅ Password updated successfully for user '{username}'")
else:
    print(f"❌ User '{username}' not found")

conn.close()

