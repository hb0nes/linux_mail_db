# Readme
Linux MailDB will create an in-memory DB (using FxHashMap) by reading the configured mail files once upon starting, and then tailing the files for further updates.
This DB can then be queried. Made to assist sys admins or support teams in debugging mail issues.
