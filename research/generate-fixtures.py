"""Development-only independent reference fixtures; contains synthetic data only."""
import ctypes
import hashlib
import hmac
import json
from pathlib import Path
import sqlite3
import struct
import subprocess
import sys
from Crypto.Cipher import AES

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / '.cache' / 'reference-fixtures'
WORK.mkdir(parents=True, exist_ok=True)
FIXTURES = ROOT / 'test' / 'fixtures'
FIXTURES.mkdir(parents=True, exist_ok=True)

def tlv(tag, data):
    return bytes([tag]) + struct.pack('<H', len(data)) + data

def qq_message(t, r, text):
    font = '宋体'.encode('utf-16le')
    return bytes(8) + struct.pack('<III', t, r, 0) + bytes([12, 0, 0, 0]) + struct.pack('<H', len(font)) + font + bytes(2) + tlv(1, tlv(1, text.encode('utf-16le'))) + tlv(18, tlv(1, '测试用户'.encode('utf-16le')))

def qq_info(name):
    field = 'strRecieverShowName'.encode('utf-16le')
    value = name.encode('utf-16le')
    return b'\x08' + struct.pack('<H', len(field)) + bytes(v ^ (255-len(field)) for v in field) + struct.pack('<I', len(value)) + bytes(v ^ (255-len(value)) for v in value)

qq_path = WORK / 'qq-plain.db'
if qq_path.exists(): qq_path.unlink()
conn = sqlite3.connect(qq_path)
conn.execute('PRAGMA page_size=8192')
for kind, peer, name in [('group', '123456', '测试群'), ('buddy', '55555', 'Alice')]:
    conn.execute(f'CREATE TABLE {kind}_{peer}(Time INTEGER,Rand INTEGER,SenderUin INTEGER,MsgContent BLOB,Info BLOB,PRIMARY KEY(Time,Rand))')
    for i, text in enumerate(['第一条 hello', '第二条 中文搜索', '第三条 world']):
        conn.execute(f'INSERT INTO {kind}_{peer} VALUES(?,?,?,?,?)', (1700000000+i, i+1, 10001+i, qq_message(1700000000+i,i+1,text),qq_info(name)))
conn.commit(); conn.close()
oracle = ROOT / '.cache' / 'qq-oracle.exe'
dll = Path(sys.argv[1])
output = WORK / 'qq-pages.bin'
subprocess.run([str(oracle),str(dll),str(qq_path),str(output)],check=True,capture_output=True)
extended = b'SQLite header 3\0' + bytes(1024-16)
(FIXTURES/'qq.json').write_text(json.dumps({'plain':qq_path.read_bytes().hex(),'encrypted':(extended+output.read_bytes()).hex(),'account':'10000','syntheticKeyByte':66},indent=2),encoding='utf8')

sqlite_dll = Path(sys.base_prefix)/'DLLs'/'sqlite3.dll'
sql = ctypes.CDLL(str(sqlite_dll))
sql.sqlite3_open.argtypes=[ctypes.c_char_p,ctypes.POINTER(ctypes.c_void_p)]
sql.sqlite3_exec.argtypes=[ctypes.c_void_p,ctypes.c_char_p,ctypes.c_void_p,ctypes.c_void_p,ctypes.c_void_p]
sql.sqlite3_file_control.argtypes=[ctypes.c_void_p,ctypes.c_char_p,ctypes.c_int,ctypes.c_void_p]
sql.sqlite3_close.argtypes=[ctypes.c_void_p]
def wx_db(name,script):
    file=WORK/(name.replace('/','_'))
    if file.exists():file.unlink()
    handle=ctypes.c_void_p()
    assert sql.sqlite3_open(str(file).encode(),ctypes.byref(handle))==0
    reserve=ctypes.c_int(80)
    assert sql.sqlite3_file_control(handle,b'main',38,ctypes.byref(reserve))==0
    assert sql.sqlite3_exec(handle,('PRAGMA page_size=4096;'+script).encode(),None,None,None)==0
    assert sql.sqlite3_close(handle)==0
    plain=file.read_bytes()
    assert plain[20]==80
    key=bytes([36])*32;salt=hashlib.sha256(name.encode()).digest()[:16]
    mac_key=hashlib.pbkdf2_hmac('sha512',key,bytes(b^0x3a for b in salt),2,32)
    output=bytearray()
    for page_no in range(1,len(plain)//4096+1):
        page=plain[(page_no-1)*4096:page_no*4096];start=16 if page_no==1 else 0
        iv=hashlib.sha256(f'{name}/{page_no}'.encode()).digest()[:16]
        body=AES.new(key,AES.MODE_CBC,iv).encrypt(page[start:4016])
        mac=hmac.new(mac_key,body+iv+struct.pack('<I',page_no),'sha512').digest()
        output.extend((salt if page_no==1 else b'')+body+iv+mac)
    return {'path':name,'plain':plain.hex(),'encrypted':output.hex(),'salt':salt.hex()}

chat='room@chatroom';table='Msg_'+hashlib.md5(chat.encode()).hexdigest()
contact="""CREATE TABLE contact(id INTEGER PRIMARY KEY,username TEXT,nick_name TEXT,remark TEXT,alias TEXT,description TEXT,small_head_url TEXT,big_head_url TEXT,verify_flag INTEGER,local_type INTEGER);
INSERT INTO contact VALUES(1,'wxid_me','本人','','','','','',0,0),(2,'wxid_a','Alice','','','','','',0,0),(3,'room@chatroom','测试群','','','','','',0,0);
CREATE TABLE chat_room(id INTEGER,owner TEXT);INSERT INTO chat_room VALUES(3,'wxid_a');
CREATE TABLE chatroom_member(room_id INTEGER,member_id INTEGER);INSERT INTO chatroom_member VALUES(3,1),(3,2);"""
session="""CREATE TABLE SessionTable(username TEXT,unread_count INTEGER,summary TEXT,last_timestamp INTEGER,last_msg_type INTEGER,last_msg_sender TEXT,last_sender_display_name TEXT);INSERT INTO SessionTable VALUES('room@chatroom',2,'最新摘要',1700000002,1,'wxid_a','Alice');"""
messages=f"""CREATE TABLE Name2Id(user_name TEXT);INSERT INTO Name2Id VALUES('wxid_me'),('wxid_a'),('room@chatroom');
CREATE TABLE {table}(local_id INTEGER PRIMARY KEY,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content BLOB,WCDB_CT_message_content INTEGER);
INSERT INTO {table} VALUES(1,1,1700000000,2,'wxid_a:\n第一条 hello',0),(2,1,1700000001,1,'wxid_me:\n第二条 中文搜索',0),(3,49,1700000002,2,'<msg><appmsg><type>6</type><title>文档.txt</title></appmsg></msg>',0);"""
favorites="""CREATE TABLE fav_db_item(local_id INTEGER,type INTEGER,update_time INTEGER,content TEXT,fromusr TEXT,realchatname TEXT);INSERT INTO fav_db_item VALUES(1,5,1700000000,'<favitem><pagetitle>测试文章</pagetitle><pagedesc>收藏摘要</pagedesc></favitem>','wxid_a','room@chatroom');"""
files=[wx_db(name,script) for name,script in [('contact/contact.db',contact),('session/session.db',session),('message/message_0.db',messages),('favorite/favorite.db',favorites)]]
(FIXTURES/'wechat.json').write_text(json.dumps({'syntheticKeyByte':36,'chat':chat,'files':files},indent=2),encoding='utf8')
print(json.dumps({'qqBytes':qq_path.stat().st_size,'wechatFiles':len(files)}))
