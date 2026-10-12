import importlib.util,sys,json,re
from pathlib import Path
repo=Path('/Users/chenhan/c9r-io/deck')
spec=importlib.util.spec_from_file_location('pi_native_review',repo/'scripts/pi_native.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def read(self):
 with (self.run.logs/f'tap-{self.number}.raw-stdout').open('w') as raw:
  for line in self.process.stdout:
   raw.write(line);raw.flush()
   prefix,event_marker,event_text=line.partition('@tap ')
   if not event_marker: continue
   if prefix and not re.fullmatch(r'test agent_pi::tests::native::tap \.\.\. ',prefix):
    raise RuntimeError('unexpected event prefix '+repr(prefix))
   event=json.loads(event_text)
   with self.lock:
    self.events.append(event);self.log.write(json.dumps(event)+'\n');self.log.flush()
m.Tap.read=read
sys.exit(m.main())
