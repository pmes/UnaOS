with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

# Let's inspect the actual signature in the current file first
print(repr(code[code.find('pub fn bootstrap('):code.find('pub fn bootstrap(')+250]))
