import sys
import weasyprint
weasyprint.HTML(filename=sys.argv[1]).write_pdf(sys.argv[2])
