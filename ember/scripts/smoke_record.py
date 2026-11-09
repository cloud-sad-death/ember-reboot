import os,pty,time,select,sys
pid,fd=pty.fork()
if pid==0:
    os.environ.update(HOME='/app/ember/tmp-home',EMBER_DATA='/app/ember/tmp-home/data',EMBER_CONFIG='/app/ember/tmp-home/ember.toml')
    os.execv('/app/ember/target-core/release/ember-cli',['ember-cli','record'])
def rd(t):
    out=b'';end=time.time()+t
    while time.time()<end:
        r,_,_=select.select([fd],[],[],0.1)
        if r:
            try: out+=os.read(fd,65536)
            except OSError: break
    return out
rd(2)
os.write(fd,b"for x in /etc/hostname /etc/os-release; do head -1 $x; done\r"); time.sleep(1)
os.write(fd,b"exit\r"); out=rd(2)
print(out.decode(errors='replace')[-300:])
